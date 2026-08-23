# Memory MVP Handoff

> Status: Active working record。每个阶段完成后记录范围、契约、验证、风险和下一阶段入口。
> Hard stop: 北京时间 2026-08-24 04:00。到时停止新增开发，收敛当前阶段并提交阶段性验收。

## 基线

- Branch: `feature/memory/cosh-ng-integration`
- Base: `3b7f6ebc5a463d968db7cf049a5e0e10b9ab3478`
- Components: `src/agent-memory` 为实现主体，Cosh-ng 仅通过既有 Extension Hook 接入。
- User loop: capture、recall、inject、explain、resume。
- Invariant: Cosh adapter 只依赖 versioned Memory Protocol；ManT 是可卸载 Provider。

## 阶段状态

| 阶段 | 状态 | Commit | 下一阶段入口 |
|---|---|---|---|
| 1. Memory Protocol | Complete | This commit | RuntimeAdapter 只消费协议类型 |
| 2. Cosh RuntimeAdapter | Pending | — | Hook 生命周期映射完成 |
| 3. Typed Local Backend | Pending | — | ContextView 可持久化与解释 |
| 4. ManT Provider | Pending | — | Provider 可卸载、可失效 |
| 5. Cosh UX | Pending | — | 30 秒体验路径 |

## 基线验证

- `cargo test --locked --lib`
- Result: 191 passed, 0 failed, 0 ignored。
- Rust: `rustc 1.97.1 (8bab26f4f 2026-07-14)`。
- 未运行 Cosh-ng workspace 全量 gate；当前尚未修改 Cosh-ng 代码。

## 保留的用户文件

仓库根目录存在与本任务无关的未跟踪答辩材料。本任务不读取、不修改、不暂存这些文件。

## 阶段 1 Handoff：Memory Protocol

### 已完成

- 新增 provider、transport、Runtime 均无关的 `anolisa.agent-memory` v1 typed protocol。
- 定义 tenant/team/user/agent/session/workspace identity、correlation、贯穿 backend 的 deadline、
  capability negotiation 和稳定 error code。
- 定义有 item、byte、token 三重预算的 ContextView，以及可解释 RecallTrace。
- 定义 append-only Runtime event、幂等 capture、带 revision 和重放 key 的 TaskState checkpoint。
- 定义 retrieved 与 admitted/dropped/usefulness 分离的 recall outcome。
- 新增 deterministic ephemeral conformance backend 和独立 JSONL stdio binding。
- JSONL 输入和输出单帧上限均为 1 MiB，超限时返回 typed error 并继续后续请求。
- 新增 JSON Schema 输出、golden fixtures、中英文正式设计文档。

### 关键契约

- Adapter 只能依赖 `MemoryBackend` 和 protocol types，不能依赖 SQLite、ManT 或 MCP tool 名。
- identity 必须由可信 host 填入，缺失或不兼容时不回退到 shared scope。
- 同一 idempotency key 和相同 mutation 返回 replay；相同 key 不同内容返回 conflict。
- TaskState 新建 revision 为 1，更新必须携带上一 revision，陈旧写入返回 conflict。
- Mutation ack 明确 `process_local` 或 `durable`；恢复 ContextItem 携带 task revision。
- backend 返回 candidate 不等于命中，Runtime 必须另报 admitted/dropped，helpful 可为 unknown。
- RecallTrace 沿用 Runtime trace ID、按 session 隔离，并要求 returned items 被完整分到
  admitted 或 dropped。
- 最终模型结果只有 `turn_committed` event；`AfterModel`/`Stop` 不得伪装为已提交结果。

### 验证

- `cargo test --locked --test protocol_test --test backend_wire_test`
  - 18 passed；另有 backend binary 单测 1 passed。
  - 覆盖 wire schema、golden fixture、late deadline、capability forward compatibility、
    tenant/session isolation、预算后置校验、mutation replay、revision recovery、完整 outcome、
    typed forget、未知 operation 和双向超限帧恢复。
- `cargo test --locked`
  - 独立测试 Agent 首轮验证 362 passed；最终协议加固后复跑 365 passed，0 failed，0 ignored。
- `cargo clippy --locked --lib --bins --tests -- -D warnings`
- `RUSTDOCFLAGS='-D warnings' cargo doc --locked --no-deps`
- `git diff --check`

### 已知边界与阶段 2 入口

- Ephemeral backend 仅用于 conformance，不承诺持久化。
- Token 是保守估算，后续由 Runtime 同时记录 provider actual usage。
- 同步 trait 可以丢弃 late result，但无法抢占任意 backend code；process/network adapter 仍需
  在 I/O boundary 实施 cancellation。
- Capability 使用保留原名的开放字符串；response/error 支持 additive field。
- Session close 可以安全重放；idempotency alias、primary object 和 RecallTrace 均有硬上限。
- 阶段 2 实现 Cosh RuntimeAdapter，并坚持用户任务 fail-open、Memory identity fail-closed。
- 任何尚未由 Cosh commit 的 model output 都不得成为 Fact 或 TaskState。
