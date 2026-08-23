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
| 1. Memory Protocol | Complete | `c2dfd0b1` | RuntimeAdapter 只消费协议类型 |
| 2. Cosh RuntimeAdapter | Complete | `5dc23c73` | 以 durable backend 替换开发期 backend |
| 3. Typed Local Backend | Complete | This commit | ContextView 可持久化与解释 |
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

## 阶段 2 Handoff：Cosh RuntimeAdapter

### 已完成

- 新增仅依赖 `MemoryBackend` 的 `CoshRuntimeAdapter<B>`，backend 可以在不改 Hook
  映射的情况下替换。
- `SessionStart` 执行 open + `session_resume` recall；`UserPromptSubmit` 执行幂等懒
  open + raw logical prompt 的 `turn` recall。
- `PostToolUse` 和 `PostToolUseFailure` 先懒打开 session，再以 session/run/tool
  维度幂等捕获脱敏、有界、带 ref/hash 的不可变证据。
- `AfterModel` 和 `Stop` 明确为零 capture，也没有注册到 extension manifest。
- 整次 Hook 共用 trace ID 和绝对 deadline。同步 backend 放到 worker thread，
  adapter 按 deadline 返回 fail-open 结果，不继续阻塞用户 turn。
- ContextView 经过二次 item/byte admission、secret redaction、prompt-injection
  quarantine 和固定 untrusted-data wrapper，再转成 `additionalContext`。
- 以实际注入结果上报 admitted/dropped；outcome telemetry 失败不会丢掉已经
  通过安全 admission 的上下文。
- 新增 one-shot stdio Hook binary 和 strict v1 extension manifest，作为阶段 3 的打包
  输入。
- 新增中英文 RuntimeAdapter 设计文档。

### 信任与降级契约

- 管理面 identity 由宿主配置；model 和 event data 不能覆盖 tenant/team/user/
  agent/workspace。
- 本地 binary 使用 effective Unix UID 作 principal，canonical cwd 的指纹只是
  本地 workspace scope key，不是 B 端 ACL。
- Memory 身份与数据访问 fail-closed；用户任务对 backend unavailable、deadline、
  malformed frame 和 capture failure fail-open。
- 召回内容永远是 data，不会作为裸 system instruction。
- 阶段 2 不把 process-local backend 宣传成跨 Hook 记忆，所以 Makefile/RPM 暂不
  安装 Hook binary 与 manifest。阶段 3 接通 durable backend 后再开放包装。

### 验证

- `cargo test --locked --test cosh_adapter_test --test cosh_hook_wire_test`
  - 11 passed，覆盖 lazy activation、raw prompt、上下文脱敏/转义/隔离、证据
    幂等、pre-commit 零 capture、真实 deadline 切断、空/错误/超长 stdio frame。
- `cargo test --locked`
  - 376 passed，0 failed，0 ignored。
- `cargo clippy --locked --lib --bins --test cosh_adapter_test --test cosh_hook_wire_test -- -D warnings`
- `RUSTDOCFLAGS='-D warnings' cargo doc --locked --no-deps`
- `cargo fmt --all -- --check`
- `git diff --check`
- 独立测试 Agent 用当前 `cosh-core` strict v1 registry 载入真实 manifest，确认
  schema v1、health healthy、0 diagnostics 和 4 个 Hook capability。
- 独立架构 review 提出的 process-local 假闭环、同步阻塞和自然语言注入三个
  P1 已通过“暂不发布 + lazy open”、worker deadline 和 quarantine 关闭。

### 阶段 3 入口

- 新增独立 typed SQLite backend，不复用旧 Markdown/BM25 store 的隐式语义。
- 需要 WAL、schema version、事务 ACK、修订冲突、幂等 replay、RecallTrace/outcome
  持久化以及 process reopen/cold-resume 测试。
- Hook binary 默认打开 local backend，然后才在 Makefile/RPM 中安装 binary 和
  `/usr/share/anolisa/extensions/anolisa.agent-memory/cosh-extension.json`。

## 阶段 3 Handoff：Typed Local Backend

### 已完成

- 新增独立的 `LocalMemoryBackend`，以 SQLite schema v1 持久化 session、不可变
  Runtime event、TaskState、ContextView、RecallTrace、outcome 和 close record。
- 默认数据库位于 `$XDG_STATE_HOME/anolisa/agent-memory/memory-v1.sqlite3`；可信
  launcher 可用 `ANOLISA_MEMORY_DB` 指定路径，但不能以该变量提供 identity。
- 数据库父目录和文件分别强制为 `0700`、`0600`，拒绝 database symlink；SQLite
  启用 WAL、foreign keys、5 秒 busy timeout 和 `synchronous=FULL`。
- 首次 schema 创建在 `BEGIN IMMEDIATE` 中串行化，并在 WAL 切换前完成；50 次
  双线程并发首次打开压力测试均通过。
- Mutation 主对象和幂等 key 在同一事务提交；event alias 与 outcome alias 均有
  每对象硬上限，避免用不同 replay key 绕过容量限制。
- TaskState 使用 expected revision 防止并发覆盖；Verified TaskState 优先于
  Candidate tool evidence 进入同一个 item/byte/token budget。
- 工具证据可跨同 workspace 的 session 召回。普通 turn 要求 query token overlap，
  session resume 按最近 evidence 恢复；每项保留 DB row/event provenance。
- Hook binary 默认打开 durable local backend。Make、RPM 和组件 manifest 安装
  backend binary、Hook binary 与 Cosh extension manifest，跨进程 capture/recall
  正式形成闭环。
- 新增中英文 typed local backend 设计文档。

### 持久化与恢复契约

- 成功的 mutation ACK 表示 SQLite durable commit，不表示 provider KV cache 已保存。
- Kill Hook 后新进程可以重放丢失 ACK、恢复 TaskState revision、召回相关工具证据，
  并读取原 session 的 RecallTrace/outcome。
- Raw tool output 不直接写入长期记忆；只保存脱敏、有界 summary、hash/ref 与 outcome。
- Diagnostic ContextView 保留 7 天；closed session 和原始 Candidate event 保留 30 天；
  reviewed TaskState 保留到显式 forget。配额压力只淘汰最旧 view 与 closed session。
- 进程、PTY、fd 和 in-flight tool outcome 没有可靠证据时仍为 unknown。
- 新版本 schema 会 fail closed；v1 尚未提供自动 schema migration。

### 验证

- `cargo test --locked`
  - 388 passed，0 failed，0 ignored；其中 local backend 12 项。
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --locked --no-deps`
- `cargo fmt --all -- --check`
- `git diff --check`
- 并发首次打开测试连续运行 50 次，全部通过。
- 真实 one-shot Hook 进程 A capture 工具证据，进程退出后进程 B 在相同 workspace
  召回 Candidate evidence；相关 query 命中、无关 query 不命中。
- `make build` release 构建通过。`make install INSTALL_PROFILE=system PREFIX=/usr`
  在隔离 staging root 中安装三个 `0755` binary 和一个 `0644` Cosh manifest。
- 独立 Stage 3 review 提出的 close replay lifecycle、无回收导致 recall 永久耗尽、
  ANOLISA contract 与 RPM/Make 目录不一致三个 P1 均已修复并回归。

### 阶段 4 入口

- ManT 只能作为可选 `KnowledgeProvider`，不得成为 protocol、RuntimeAdapter 或
  local memory 的必需依赖。
- Provider unavailable 时保留 local TaskState/evidence，并把 recall 标记为 degraded；
  禁止静默把失败伪装成完整召回。
- 长期库只保留 ManT 文档 ref、selector、fingerprint 和必要的有界 excerpt，不能复制
  整套手册成为第二真源。
