# Memory MVP Handoff

> Status: Active working record。每个阶段完成后记录范围、契约、验证、风险和下一阶段入口。
> Hard stop: 北京时间 2026-08-24 04:00。到时停止新增开发，收敛当前阶段并提交阶段性验收。

## 基线

- Branch: `feature/memory/cosh-ng-integration`
- Base: `3b7f6ebc5a463d968db7cf049a5e0e10b9ab3478`
- Components: `src/agent-memory` 为实现主体；Cosh-ng Hook wire 仅 additive 增加固定
  `workspace_root`，其余 Memory 语义留在 adapter/protocol/backend。
- User loop: capture、recall、inject、explain、resume。
- Invariant: Cosh adapter 只依赖 versioned Memory Protocol；ManT 是可卸载 Provider。

## 阶段状态

| 阶段 | 状态 | Commit | 下一阶段入口 |
|---|---|---|---|
| 1. Memory Protocol | Complete | `c2dfd0b1` | RuntimeAdapter 只消费协议类型 |
| 2. Cosh RuntimeAdapter | Complete | `5dc23c73` | 以 durable backend 替换开发期 backend |
| 3. Typed Local Backend | Complete | `1af8136c` | ContextView 可持久化与解释 |
| 4. ManT Provider | Complete | `63d3ea48` | Provider 可卸载、可失效 |
| 5. Cosh UX | Complete | This commit | 30 秒体验路径与验收报告 |

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
  - 389 passed，0 failed，0 ignored；其中 local backend 12 项。此前 trace 手工合计
    误记为 388，本阶段按 Cargo 各 test binary 输出复核并更正。
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

## 阶段 4 Handoff：可卸载 ManT KnowledgeProvider

### 已完成

- 新增 provider-neutral `KnowledgeProvider` SPI，包含 typed descriptor、health、focused
  query、bounded item 和稳定 error code；核心协议、Cosh adapter 与 SQLite schema 都不
  依赖 ManT 类型。
- 新增 ManT v0.9 native one-shot JSON adapter。每次查询精确协商
  `mant.cli/request/excerpt/search v0.9`，只支持 literal search、single-entry explain
  和 section excerpt，没有 whole-document 操作。
- ManT executable 由可信 host 显式提供或从 PATH 发现；Agent Memory 不安装、下载、
  更新 ManT，也不执行 shell。缺失等价于 provider 未加载。
- Native request 不超过 65,536 bytes；stdin/stdout/stderr、selector、item 与 excerpt
  都有硬限。stdout/stderr 并发排空，超时 kill 独立 process group，stderr、物理路径、
  query 和文档内容不进入 safe error。
- `LocalMemoryBackend` 接受可替换 provider binding。普通 turn 以 TaskState、focused
  knowledge、tool evidence 的顺序共享同一 item/byte/token budget，并持久化为一个
  ContextView 与 RecallTrace。
- Provider 失败时 view 使用 `local_only_knowledge_degraded` 和 typed reason，本地
  TaskState/evidence 继续返回；成功时使用 `local_with_knowledge`。
- Provider 内容保持 `Knowledge + Candidate + untrusted-data`，再次经过 secret redaction、
  injection quarantine 与 Runtime admission，不能因 ManT 解析而升级为 Verified。
- Cosh Hook 自动接入已安装的 `mant`。`ANOLISA_MANT_PATH` 选择 executable，
  `ANOLISA_MEMORY_MANT_DOCUMENT` 选择 logical document，
  `ANOLISA_MEMORY_MANT=off` 可显式卸载。
- 新增中英文 provider 设计文档，并以官方 ManT v0.9 protocol/schema/CLI 文档完成
  一手资料核对。

### 验证

- Stage 4 scope 的测试总数 403，0 failed，0 ignored；全工作树 gate 还包含已并行完成但
  尚未进入本提交的 5 项 Stage 5 CLI test，因此 Cargo 实际输出 408 passed。
- `knowledge_provider_test` 10 项，覆盖 fake provider、官方 v0.9 search/explain wire、
  合法零命中、嵌套 outline、typed IR、malformed response、process-group/I/O deadline
  以及 request/output/aggregate bounds。
- `local_backend_test` 15 项，新增 provider merge、local-only degraded fallback，并验证
  session/scope 授权在任何外部 provider 调用之前完成。
- `cosh_hook_wire_test` 4 项，新增真实 Hook 进程连接 fake ManT v0.9 的端到端 recall。
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --locked --no-deps`
- `cargo fmt --all -- --check`
- `git diff --check`
- 独立 review 提出的宽松 response JSON、零命中误判 degraded、stdin write 未纳入
  deadline、leader 退出后 descendant 持 fd、outline ancestor 误校验、excerpt payload
  未强类型化、未授权请求先触发外部查询等问题均已关闭并回归；最终 P0/P1 复核清零。

### 已知边界与阶段 5 入口

- 当前 task policy 每轮选择一个 logical document 和一个聚焦 literal；多 Provider 可以
  在同一 SPI 后替换，但本地 broker v1 同时只绑定一个，避免 500 ms Hook deadline 内
  出现无界 fan-out。
- ManT adapter 每次 query 都重新 probe，没有 cache；FNV-1a 只用于 staleness，不是
  密码学完整性证明。
- Cosh 用户需要看到 recall 条数、token、view ID，并能用独立管理 CLI 运行
  status/doctor/demo/why/forget；管理面不能要求用户猜 owning session。

## 阶段 5 Handoff：Cosh 用户体验与管理 CLI

### 已完成

- 新增正式安装的 `agent-memory-ctl`，提供 `status`、`doctor`、`demo`、`why` 和
  `forget`，每个 command 都支持 `--json`。
- `doctor` 检查私有 local store、必需的 Cosh Hook、可选 Cosh runtime，并对 PATH 或
  `ANOLISA_MANT_PATH` 中的 ManT 执行真实 v0.9 protocol probe；ManT 缺失不影响健康。
- `demo` 捕获唯一的合成 evidence，drop 并冷重开 backend，在第二个 session 中召回，
  记录 Useful outcome，并打印 `cold_reopen_ms`、ContextView ID 和可复制的 `why` command。
- Cosh 只有在安全 admission 后才显示固定的 user notice，内容限于 item 数、估算 Token、
  view ID 和 `why` command；backend failure 或空 recall 不显示误导性命中。
- 新增可信 local management scope。`why` 和 `forget` 由 backend 根据 euid、Agent 和
  canonical workspace fingerprint 解析 owner，不要求 CLI 猜 session；foreign scope
  与 absent 对用户不可区分。
- Cosh Hook wire additive 携带固定 project root，与可变 shell cwd 分离。Git worktree
  内统一使用 canonical root，因此在 repo 子目录 capture、另一子目录 recall、repo root
  执行 `why/forget` 都属于同一 scope；相邻 workspace 不串。
- `forget` 必须显式 `--yes`。ContextView 删除会级联清理 RecallTrace/outcome；跨 session
  event ID 歧义返回 Conflict，不会任选一条删除。
- `status` 报告 SQLite logical/physical bytes、对象数/硬容量、7/30 天生命周期，以及
  最近 1000 个 ContextView 的 backend admission、reported outcome 和 useful outcome；
  synthetic CLI view 单独计数并从真实漏斗排除。
- Cargo、Make install/uninstall 和 RPM `%install/%files` 都纳入 `agent-memory-ctl`。
- ANOLISA raw/registry component layout 同步纳入 `bin/agent-memory-ctl`，确保
  `anolisa install agent-memory` 与 RPM/Make 三条安装路径一致。
- 新增仓库级中英文 Cosh Agent Memory 用户指南，并更新 Agent Memory README 和主用户
  指南入口；完整验收报告保存在 `.codex/memory-mvp/acceptance-report.md`。

### 指标语义

- `recall_with_items` 是 backend ContextView 中有 item 的 view，不是“最终回答用了记忆”。
- `reported_outcomes` 只统计 Runtime 已明确上报的 outcome；`useful_outcomes` 不把 unknown
  当作命中。Cosh 自动路径当前可靠上报 admitted/dropped，demo 才主动标 Useful。
- `cold_reopen_ms` 只测 SQLite backend drop/open；不代表 PTY、process、fd、model state
  或 provider KV cache 的完整 cold Agent 恢复。
- Provider API 下真实 KV cache GB 仍是 `unknown(provider-managed)`，不得由 token 数伪算。

### 验证

- `cargo test --locked`
  - 413 passed，0 failed，0 ignored；其中 local backend 18 项、CLI process 7 项、Cosh
    adapter 9 项、真实 Hook wire 4 项、ManT provider 10 项。
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --locked --no-deps`
- `cargo fmt --all -- --check`
- `python3 scripts/docs-link-check.py`
- `cargo test --locked --package cosh-core` 的 fixed workspace-root 与 safe extension reload
  两项 targeted test 均通过；`cargo clippy --locked --package cosh-core --all-targets --
  -D warnings` 通过。
- `cargo test --locked --package anolisa-core
  manifest::tests::existing_manifests_still_parse_after_schema_extension -- --exact` 通过。
- 隔离 `DESTDIR` release install smoke：installed `agent-memory-ctl` 依次运行
  `doctor -> demo -> status` 成功，随后 `make uninstall` 确认 binary 被删除。
- `rpmspec/rpmbuild` 在当前环境不存在，因此真实 RPM build 未运行；spec 的 install/files
  静态 contract 已验证。
- `git diff --check`
- 独立 Stage 5 review 提出的 raw install 漏装 CLI、synthetic demo 污染真实命中漏斗、
  可变 cwd 分裂 workspace，以及 safe extension reload 丢失 fixed root 四个 P1 均已
  关闭；最终 P0/P1 复核清零。

### 最终已知边界

- Cosh 还没有 durable `TurnCommitted` + outbox。本阶段不会把 `AfterModel` 或 `Stop`
  候选输出写成最终 Fact/TaskState，也不宣称“自动保存最终回答”。
- Local management identity 适用于可信单机 host。B 端需由 gateway 注入 tenant/team/
  user/agent/workspace principal，并使用服务端 ACL，不能复用本地 argv/env 作为授权。
- Local broker v1 同时绑定一个 KnowledgeProvider；SPI 支持替换，后续多 Provider fan-out
  需要独立 deadline、配额、排名归一化和贡献 trace。
- 下一阶段优先做 TurnCommitted durable outbox、真实 Cosh task benchmark、remote backend/
  ACL，以及 cold recovery 和 Memory-assisted Task Success dashboard。
