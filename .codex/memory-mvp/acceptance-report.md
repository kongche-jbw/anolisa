# Agent Memory 五阶段重构验收报告

## 验收结论

本分支已经完成计划中的前五个阶段，形成了一条可被 cosh-ng 实际调用、又不与
cosh-ng、ManT、MCP 或某一种存储实现绑死的 Memory 基础设施最小闭环。

闭环包含：可信身份和版本化协议、Cosh RuntimeAdapter、持久化本地后端、可卸载
ManT KnowledgeProvider、用户可见提示和管理 CLI。Memory 故障不会阻断 Cosh 当前
任务；身份或 scope 无法确认时不会回退到 shared 数据。

- Branch: `feature/memory/cosh-ng-integration`
- Base: `3b7f6ebc5a463d968db7cf049a5e0e10b9ab3478`
- Platform: Linux
- Component version: `agent-memory 0.2.6`
- Final gate: 413 tests passed，0 failed，0 ignored

五个阶段各自对应一个本地原子提交：

1. `c2dfd0b1` `feat(memory): add backend protocol v1`
2. `5dc23c73` `feat(memory): add cosh runtime adapter`
3. `1af8136c` `feat(memory): add durable local backend`
4. `63d3ea48` `feat(memory): add optional ManT provider`
5. This commit: `feat(memory): add Cosh memory controls`

## 五阶段交付

| 阶段 | 交付结果 | 验收重点 |
|---|---|---|
| 1. Memory Protocol | provider、transport、Runtime 无关的 v1 typed contract | 版本协商、强隔离、幂等、deadline、预算、RecallTrace、outcome |
| 2. Cosh RuntimeAdapter | SessionStart/UserPromptSubmit recall，工具 evidence capture | fail-open、安全注入、500 ms 用户可见 deadline、pre-commit 输出不入库 |
| 3. Typed Local Backend | SQLite schema v1 durable backend | WAL、FULL sync、revision、lost-ack replay、冷进程召回、TTL 与容量 |
| 4. ManT Provider | 可卸载的 native ManT v0.9 adapter | 精确协议协商、focused query、bounded I/O、typed degradation |
| 5. Cosh UX | `agent-memory-ctl`、Cosh notice、安装包和中英文指南 | status/doctor/demo/why/forget、指标、权限、30 秒体验 |

核心依赖方向保持单向：

```text
cosh-ng Hook -> CoshRuntimeAdapter -> Memory Protocol -> Local/Remote Backend
                                                `-> optional KnowledgeProvider
                                                      `-> ManT v0.9
```

MCP、stdio、Unix socket、HTTP 只是可以替换的 transport binding。Cosh Hook 名称没有
进入核心协议；ManT 也只是首个 KnowledgeProvider。替换 Runtime、transport、存储或
知识源不需要改写其余层的语义契约。

## 30 秒路人体验

安装时间取决于网络和 package manager，不计入 30 秒。安装完成后，在平时使用 Cosh
的 workspace 中执行：

```bash
agent-memory-ctl doctor
agent-memory-ctl demo
agent-memory-ctl status
```

`doctor` 应显示 local store 和 Cosh Hook 为健康，ManT 可以是 `not_found`；它是可选
Provider。`demo` 会捕获一条合成 evidence，关闭并冷重开 SQLite backend，在第二个
session 中召回它，并打印类似下面的结果：

```text
Agent Memory demo succeeded.
Captured: 1 synthetic event
Recalled: 1 items, including 1 candidate evidence item
Outcome: useful
Cold backend reopen: 0 ms
Context view: local-ctx-1
Explain it: agent-memory-ctl why local-ctx-1
```

复制最后一条 `why` 命令即可查看 item ID、rank、准入原因、degraded 状态和 outcome；
不会打印 Memory 内容或数据库路径。需要删除这次诊断 view 时显式确认：

```bash
agent-memory-ctl forget context-view local-ctx-1 --yes
```

安装命令仍按仓库统一入口执行：

```bash
anolisa install agent-memory
```

完整说明见 `docs/user-guide/{en,zh}/token-saving/cosh-agent-memory.md`。

## C 端使用方式

面向个人用户，产品入口是“自动工作、可以看见、可以解释、可以删除”。

1. 安装 package 后，cosh-ng 自动发现 extension，无需复制 MCP JSON。
2. 新 session 开始或提交 prompt 时，Cosh 在相同 Unix user、Agent 和 workspace scope
   中召回有界 evidence。
3. 有内容真正进入模型 context 时，界面显示 item 数、估算 Token 和 ContextView ID。
4. 用户用 `why` 检查选择依据，用 `forget --yes` 删除当前 workspace 的 task、event 或
   ContextView。
5. `status` 显示空间、硬容量、7/30 天生命周期和近期召回漏斗，并把 synthetic demo
   单独列出；`doctor` 给出可操作的修复建议。

Cosh 的固定 project root 与 shell cwd 分开传递；Git worktree 内统一归一化到 canonical
root。用户在 repo 子目录工作、回到 repo root 运行 `why/forget` 时仍处于同一 scope，
相邻 workspace 则保持隔离。

这条 C 端路径不会保存完整 transcript，也不会把 `AfterModel` 或 `Stop` 中尚未提交的
候选回答写成事实。工具输出只保存脱敏、有界 summary、hash/ref 和 outcome。

## B 端使用方式

面向团队或平台接入，建议把 Memory 部署为独立 data/control plane，让 Runtime 仅做轻
adapter。

- 可信 gateway 注入 tenant/team/user/agent/session/workspace identity；模型参数不能
  覆盖 ACL scope。
- Backend 实现 versioned `MemoryBackend` contract，可使用本地 SQLite、独立 daemon、
  stdio、Unix socket 或 HTTP。客户端不能根据产品名猜 capability。
- 每个团队或 task 可加载独立 ContextPolicy/KnowledgeProvider，ManT 只负责规范手册，
  memory backend 负责经验和状态，二者分别记录 provenance 与命中。
- RecallTrace 将 candidates、backend admission、Runtime drop 和 usefulness 分开，方便
  B 端按 tenant、Agent、task 评估，不把“搜到一条”冒充任务成功。
- Provider 缺失或降级时保留同 scope 的本地 recall；认证、scope、协议不兼容则对
  Memory fail-closed，绝不降级到 shared。

当前 local management identity 只适用于单机可信 euid + canonical workspace。多租户
服务必须由可信 gateway 建立 principal，并使用真正的服务端 ACL 与 tenant-scoped
provider process/filesystem，不能照搬本地 identity derivation。

## 用户关心的三组指标

### 1. 实际容量和生命周期

`agent-memory-ctl status` 报告 session/event/task/ContextView 当前数量与硬容量、SQLite
logical/physical bytes、ContextView 7 天保留期、closed session 和 raw event 30 天
保留期。Reviewed TaskState 由显式 `forget` 管理。

Provider KV/prompt cache 与长期 Memory 是两类资源。API 模式下物理 KV GB 仍应显示为
`unknown(provider-managed)`；只能从 provider usage 计算 cached-token hit ratio。自托管
推理才应从 vLLM、SGLang 等 exporter 读取真实 block bytes、eviction 和 TTL。

Working Memory 则按每轮预算记录：

```text
available = context_window - output_reserve - stable_prefix
            - current_turn - safety_margin
memory_budget = min(policy_cap, available - recent_tail_min - task_state_min)
```

### 2. 命中率

当前本地 `status` 对最近 1000 个 ContextView 给出不会混淆的计数：backend 返回过 item
的真实 Runtime view 数，以及 Runtime 明确报告 outcome 的数量和其中 useful 的数量。
`agent-memory-ctl demo` 生成的 synthetic view 单独计数并从漏斗排除。Cosh 自动路径可以
可靠报告 admitted/dropped；没有用户或评测反馈时 helpful 保持 unknown。

后续生产看板应继续拆为 policy invocation、Recall@K/Precision@K、Runtime admission、
grounded use、end-to-end task uplift。KV cache hit ratio 必须单列，不能与 Memory recall
合成一个“命中率”。

### 3. 冷 Agent 恢复成本

`demo` 已验证 durable backend drop/reopen、第二 session recall，并报告
`cold_reopen_ms`。这只表示本地 SQLite backend 冷重开，不等价于完整 Agent 恢复。

完整生产基准仍应通过 process kill、host restart、new-agent handoff 三种 fault injection
测量 RPO、RTO-ready、rehydrate I/O、首次正确 action 前 Token/LLM 调用、semantic
fidelity 和 warm/cold success gap。PTY、process、fd、in-flight tool outcome 与 provider
KV cache 没有可靠证据时必须保持 unknown。

## 安全和降级验收

- Recall 内容在 local provider 和 Runtime 两层做 secret redaction、prompt-injection
  quarantine 和 item/byte/token budget。
- ManT 内容始终是 Candidate 和 untrusted data，不能因解析成功升级成 Verified。
- `why` 不回显内容；foreign workspace 与不存在对象统一 NotFound。
- `forget` 必须带 `--yes`；同 workspace 跨 session 的重复 event ID 返回 Conflict，不猜测。
- Hook backend unavailable、timeout 或 malformed output 时继续用户 turn。
- 无 scope、跨 tenant、协议不兼容时不返回 Memory，也不回退 shared。

## 验证证据

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --locked --no-deps`
- `cargo test --locked`：413 passed
- `python3 scripts/docs-link-check.py`：全部相对链接有效
- 隔离 `DESTDIR` 的 release install smoke：installed CLI 依次执行
  `doctor -> demo -> status`，随后 uninstall 确认 CLI 被移除
- `anolisa-core` central raw manifest parse 通过，layout 明确包含
  `bin/agent-memory-ctl -> {bindir}/agent-memory-ctl`
- cosh-core fixed workspace-root 与 safe extension reload 两项 targeted test 通过，
  cosh-core all-target Clippy 无 warning
- Cargo metadata：四个正式 binary 包含 `agent-memory-ctl`
- RPM spec 的 install/files 清单包含 `agent-memory-ctl`
- 独立 P0/P1 review 最终清零；见 handoff 与 tracing 末尾记录

## 已知边界

1. 现有 Cosh Hook 没有 durable `TurnCommitted` + outbox，因此本轮不宣称自动保存最终
   model answer；只自动保存已发生的工具 evidence。
2. v1 local broker 同时绑定一个 KnowledgeProvider，SPI 可替换但尚未做无界并行 fan-out。
3. ManT 每次查询重新 probe；FNV-1a fingerprint 只用于 staleness，不是安全完整性证明。
4. `cold_reopen_ms` 不是完整 Agent cold restore SLO，也不代表 KV cache 恢复。
5. 当前环境缺少 `rpmspec/rpmbuild`，完成了 spec 静态 contract 和 Make staged install，
   尚未完成真实 RPM 宏展开和 package build。
6. Cosh 用户可见 notice、Runtime outcome telemetry 和长期任务成功率还需要真实流量校准。

这些边界均没有被包装成已完成能力。下一阶段优先级应是 `TurnCommitted` durable outbox、
真实 Cosh task benchmark、B 端 remote backend/ACL，以及命中率与 cold recovery dashboard。
