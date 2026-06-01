# ANOLISA CLI Development Status

本文记录 `anolisa` CLI 当前开发状态、可用功能边界和下一步优先级。更新基准：

- 日期：2026-06-01
- 分支：`kongche/dev/anolisa-p1`
- HEAD：P1-F 完成（enable agent-observability 真实执行路径：download + install + state + central-log + lock）
- 定位：P1 可开发骨架；不是可安装组件的产品形态

## 当前结论

- CLI 命令面、全局参数、JSON envelope、`NOT_IMPLEMENTED` / `INVALID_ARGUMENT` 错误码已经稳定。
- 已落地的基础库包括 `EnvService`、`FsLayout`、`Catalog`、`DistributionIndex` resolver、`InstalledState v1`、`CentralLog`、install lock、backup plan skeleton、`EnablePlan` planner（P1-E2 新增）。
- 当前可真实使用的命令：`env`、`logs`、`list`、`status`、`enable <cap> --dry-run`。
- `list` 接入 bundled `Catalog` + `InstalledState v1`，支持 `--enabled`；`--available` 已布线但当前恒为 `true`，等 EnvFacts gating 落地后再收紧。每行带真实 `status` 字段（`installed | degraded | disabled | failed | adopted | not_installed`），`--enabled` 只放过 `installed | degraded | adopted`。
- `status [CAPABILITY]` 接入 `InstalledState v1`：fresh install / 未安装 capability 返回 ok 空集（或 `not_installed`），非错误。直接投出 state 里的 `component_refs` / `enabled_features` / `health`，不依赖 resolver。
- `enable <cap> --dry-run` 接入 `anolisa_core::plan_enable`：纯只读，不下载、不写 state、不写 central log。当前只在 CLI 层放行 `agent-observability`；其它 capability 返回 `NOT_IMPLEMENTED`（带明确 scope hint）。`--feature` / `--with-adapter` / `--from-source` 在 `--dry-run` 下显式返回 `NOT_IMPLEMENTED`。无 `--dry-run` 的 `enable` 继续返回 `NOT_IMPLEMENTED`。
- planner 同时评估 capability 与 **component-level** `env_requirements`：`kernel_min` / `btf` / `cap_bpf` / `libc` / `pkg_base` 都会变成 precheck 行（component 检查带 `<component>.<name>` 命名空间）。`unknown` 一律降级为 `warn` 而不是默认 ok。
- 缺失 `distribution-index/index.toml` 不再返回 `INVALID_ARGUMENT`：planner 用空 index 继续出 plan，顶层 warning + per-component `blocked_reason = "no prebuilt artifact for …"` 让用户能直接看出缺什么。
- `DistributionIndex` 查找顺序与 Catalog 对齐：`manifests_overlay/distribution-index/index.toml`（按 install mode 对应 `/etc` 或 `~/.config`）→ packaged `datadir/manifests/...` → dev-tree。overlay 当前是整文件替换，不做条目合并。
- `ComponentPlan.resolved_files` 把 manifest 模板（如 `{bindir}/agentsight`）按 `FsLayout` 渲染成真实路径，`files` 字段保留模板原文以表达 manifest intent。
- Catalog 加载已修正为分层装配：bundled = packaged `datadir/manifests`（缺时回落 dev-tree manifests），overlay = `manifests_overlay` 按 install mode 挂作 `system` 或 `user` 层叠加，不再被 overlay 替代。
- `enable agent-observability`（无 `--dry-run`，单 capability，无 `--feature` / `--with-adapter` / `--from-source`）已落地为第一条真实执行路径：通过 `DownloadCache` 拉取 artifact（当前仅 `file://`，后续 scheme 待加）、用 `InstallRunner` 将文件落到 ANOLISA-owned roots（`bin_dir` / `etc_dir` / `state_dir` / `lib_dir` / `libexec_dir` / `datadir` / `log_dir` / `cache_dir`）、写入 `InstalledState v1` 的 capability + per-component 对象（含 sha256 / services / files / OperationRecord）、按 `operation_id` 在 `CentralLog` 追加 `started` 与 `succeeded`（失败时 `failed`）、整段操作持有 `InstallLock`；任何中途失败会自清理：unlink 本次 op 写入的 ANOLISA-owned 文件、best-effort 写 `Failed` 日志、释放锁。
- 其它所有真实有副作用命令（`disable` / `restart` / `update` / `uninstall` / `subscription` / `adapter` / `self` / `runtime *` / `osbase *` / …）仍返回 `NOT_IMPLEMENTED`。
- `backup.rs` 仍是 plan-only；因此 external-file 修改类的执行路径（adapter 改第三方配置 / rollback 还原外部状态）继续禁止，本里程碑只允许 ANOLISA-owned files。

## 当前可用命令

```bash
cd src/anolisa
cargo run -- --help
cargo run -- env
cargo run -- env --json
cargo run -- logs
cargo run -- logs --json
cargo run -- logs --kind operation --severity warn --limit 10 --json
cargo run -- list
cargo run -- list --json
cargo run -- list --enabled --json
cargo run -- status
cargo run -- status agent-observability --json
cargo run -- enable agent-observability --dry-run
cargo run -- enable agent-observability --dry-run --json
cargo run -- --install-mode system enable agent-observability --dry-run
cargo run -- --install-mode user enable agent-observability
cargo run -- --install-mode user enable agent-observability --json
cargo run -- --install-mode user --verbose enable agent-observability
```

> 真实执行需要 distribution-index 里存在 host 可命中的 `agent-observability` artifact。fresh checkout 下 bundled `manifests/distribution-index/index.toml` 当前并不含 macOS / aarch64 二进制，因此在开发机上直接跑（无 overlay）会以 `INVALID_ARGUMENT` + `plan is blocked` 收尾。推荐的 P1-F smoke 流程：搭一个 overlay distribution-index 用 `file://` 指向本地 fake binary，并通过 `--install-mode system --prefix /tmp/anolisa-smoke` 让所有写入落到 tmp 目录里（system mode honors `--prefix`，user mode 不会 — 见 `FsLayout`）。

### `env`

状态：可用。

能力：

- 输出当前 OS、arch、kernel、libc、pkg_base、BTF、CAP_BPF、container、user、uid、home。
- 默认人类可读。
- `--json` 输出统一 response envelope。

限制：

- 当前 probe 仍是最小集合，后续还需要补 eBPF、systemd、包管理器、runtime framework probe。

### `logs`

状态：可用。

能力：

- 读取 `FsLayout` 解析出的 central log JSONL。
- fresh install 无日志文件时返回空数组，不报错。
- 支持：
  - `[OBJECT]`
  - `--operation-id`
  - `--kind operation|component`
  - `--source`
  - `--component`
  - `--severity debug|info|warn|error`
  - `--since`
  - `--limit`
  - `--json`

限制：

- 目前只有查询能力；组件日志 ingest、rotation、follow mode、索引加速还未实现。

### `list`

状态：可用（P1-E1 + fixup）。

能力：

- bundled `Catalog` 从 packaged `datadir/manifests` 加载（缺时回落 dev-tree manifests）；`manifests_overlay` 按 install mode 作为 `system` 或 `user` 层叠加，不再整盘替换。
- 叠加 `InstalledState v1` 得出每行真实 `status`（`installed | degraded | disabled | failed | adopted | not_installed`）+ `installed_version`。
- `--enabled` 只放过 `installed | degraded | adopted`，`disabled` 和 `failed` 被显式排除；`--available` 当前恒为 true，但 flag 已生效（未来 EnvFacts gating 直接改谓词即可）。两个 flag 同传取交集。
- `--json` 输出统一 response envelope；人类输出固定列宽 `NAME / PRIORITY / STATUS / VERSION`。
- `priority` 字段当前取自 `capability.stability`（manifest 暂无独立 priority 字段）；`summary` 取自 `capability.description`。

限制：

- `available` 不做 EnvFacts gating，必须等 capability resolver / env probing 落地。
- `components` / 健康聚合不在 `list` 范围内，留给 `status`。

### `status`

状态：可用（P1-E1 + fixup）。

能力：

- 不带参数列出 `InstalledState v1` 中所有 `kind == capability` 对象。
- 带 `[CAPABILITY]` 时过滤到精确名字；未安装返回单条 `status: not_installed`（仍 ok）。
- Fresh install 无 `installed.toml` 返回空集，非错误。
- JSON 直接投出 state 里已有字段：`components`（取自 `component_refs`）、`enabled_features`、`health`；不依赖 resolver。
- `--json` 输出 envelope；人类输出 `name / status / version / installed_at`，`--verbose` 附 `last_operation_id` / components / enabled_features / health。
- `ObjectStatus` 全集映射到 wire 上：`Installed→installed`、`Partial→degraded`、`Disabled→disabled`、`Failed→failed`、`Adopted→adopted`（与 `list` 共享 `common::object_status_str`）。

限制：

- 没有 health probe；`health` 字段仅原样回放 state 已写入的探针记录，未触达运行时。

### `enable`

状态：仅 `--dry-run` 可用（P1-E2）。

能力：

- `enable agent-observability --dry-run` 调用 `anolisa_core::plan_enable` 生成 `EnablePlan`，纯只读：不下载、不写 `InstalledState`、不写 `CentralLog`、不接触 backup。
- 解析链路：layered `Catalog`（bundled + overlay）→ capability `agent-observability` → component manifests → `EnvService::detect()` → `DistributionIndex`（overlay 优先 → packaged → dev-tree）→ resolver 按 install_mode / os / arch / libc / pkg_base / preferred_artifact_types 选 artifact。
- Precheck 覆盖 capability **与** component 两层 `env_requirements`：`os` / `arch` / `install_mode` / `libc` / `pkg_base` / `kernel_min` / `btf` / `cap_bpf`。component 层带命名空间（如 `agentsight.btf`）。`unknown`（探针不可用）一律 `warn`，从不假装 ok。
- Plan 输出涵盖：`capability` 名称 + `stability` + `install_mode`、`status`（`ready | degraded | blocked`）、`blocked_reason`、`env_facts` 摘要、`prechecks`、每个 component 的 manifest 版本 + status + 命中的 `artifact`（`artifact_type` / `backend` / `version` / `url` / `sha256`）+ `services` / `files` / `resolved_files` / `requires_privilege` / `env_requirements`、`layout` 摘要、`warnings`、`next_actions`。
- `resolved_files` 把 `{bindir}` / `{etcdir}` / `{statedir}` / `{logdir}` / `{datadir}` 模板按 `FsLayout` 渲染成绝对路径，`files` 保留原始模板。
- 缺失 distribution index 时 planner 用空 index 继续出 plan：顶层 warning + 组件 `blocked` 且 `artifact = null`，不再返回 `INVALID_ARGUMENT`。
- 状态聚合：任一 precheck `fail` 或 component `blocked` → `blocked`；任一 `warn` 或 component `degraded`、或 planner 抛出 warning（版本漂移 / 空 index / 缺 sha256）→ `degraded`；否则 `ready`。
- 即使 `blocked` 也以 exit 0 + envelope 返回，仅参数错误（未知 capability、多 capability、不支持的 flag）返回 `INVALID_ARGUMENT` 或 `NOT_IMPLEMENTED`。
- `--json` 输出统一 envelope；人类输出按 capability / env / prechecks / components / layout / warnings / next 分段，`--verbose` 额外展示 sha256 / services / files / resolved_files / requires_privilege。

限制：

- 当前 CLI 层只允许 `agent-observability`，其它 capability 显式 `NOT_IMPLEMENTED`（planner 本身已通用，扩展只需放开 scope 守卫）。
- `--feature` / `--with-adapter` / `--from-source` 在 `--dry-run` 下显式 `NOT_IMPLEMENTED`。
- 无 `--dry-run` 的 `enable` 仍返回 `NOT_IMPLEMENTED`：下载器 / install runner / transaction / backup / rollback / state + central-log 写入链路均未接通。
- `kernel_min` 比较是简单的数字前缀语义（`5.15.0-anolis23.x86_64` 取 `5.15.0`），无法解析时回退 `warn`；尚未按 OS 类型 gate（macOS host 的 `25.3.0` 在 numeric 上 >= `5.8`，但 OS precheck 已先一步把整体 plan 标 blocked）。
- DistributionIndex overlay 当前是整文件替换，不做 entry-level 合并；如果用户需要在 overlay 里追加少量 entry，需要把完整 entry 列表复制到 overlay 文件。

### `enable agent-observability` (no --dry-run)

状态：可用（P1-F）。

能力：

- CLI 形态严格收敛到：单 capability == `agent-observability`、无 `--feature` / `--with-adapter` / `--from-source`、走 `--dry-run` 同款 plan 构建链路（catalog → distribution-index → env → layout → `plan_enable`）。
- 命中 `plan.status == Ready | Degraded` 后调用 `anolisa_core::execute_enable(plan, layout, actor)`，整段执行包在 `InstallLock`（`state_dir/lock`）里：
  1. plan == `Blocked` 直接拒绝，不打开锁、不写日志、不动文件；
  2. 拿到锁后追加 `started` `LogRecord`（`kind=operation`、`status=null`、`severity=info`、`operation_id` 唯一）；
  3. 依次对每个 component 执行 `DownloadCache.fetch` → `InstallRunner.install`；
  4. 全部组件成功 → 加载 / 新建 `InstalledState v1`，upsert capability + per-component 对象（component 含 `OwnedFile { path, owner=Anolisa, sha256 }`、`ServiceRef { manager=systemd|systemd-user }`、`status=Installed`、`last_operation_id`），追加 `OperationRecord { status="ok" }` 并 `save`；
  5. 追加 `succeeded` `LogRecord`（`status=ok`、复制 plan warnings）；释放锁；
  6. 中途任一步失败：unlink 本次 op 已落盘的 ANOLISA-owned 文件、best-effort 追加 `failed` `LogRecord`（`status=failed`、`severity=error`、message 含原始 error）、释放锁、把原 error 原样返回给 CLI。
- `actor` 取 `$USER` → `$LOGNAME` → `"cli"` 兜底，统一写入 started / succeeded / failed 三类 `LogRecord` 与 `OperationRecord`。
- JSON 输出（`ExecutePayload`）：`operation_id` / `capability` / `install_mode` / `components` / `installed_files [{component, path, sha256}]` / `state_path` / `central_log_path` / `warnings`。人类输出按 `enable <cap> succeeded` / `operation_id` / `install_mode` / `components` / `installed_files (N)` / `state` / `log` / `warnings` 分段；默认 sha256 取前 8 字符，`--verbose` 渲染完整 64 位。

错误映射（P1-F 阶段统一收敛到 `INVALID_ARGUMENT` / exit 2，便于不动 `response.rs`）：

| `ExecuteError` | CLI reason 摘要 |
|---|---|
| `LockHeld { path }` | `install lock at <path> is held by another process — run again after the other invocation finishes` |
| `PlanNotExecutable { status, reason }` | `plan is <status>: <reason> — run \`anolisa enable agent-observability --dry-run\` for details and resolve blockers before retrying` |
| `MissingArtifact { component }` | `component '<c>' has no resolved artifact (catalog vs distribution-index mismatch — check ...)` |
| `Download { component, source }` | `download for component '<c>' failed: <source>`（含 `ChecksumMismatch` / `UnsupportedScheme` / IO 等） |
| `Install { component, source }` | `install for component '<c>' failed: <source>`（含 `UnsupportedArtifactType` / `ExternalPath` / IO 等） |
| `State { source }` | `installed state write failed: <source>` |
| `Log { source }` | `central log write failed: <source>` |
| `Lock { source }` | `install lock io: <source>` |

> 真正合适的 exit code 是新增 `EXECUTION_FAILED`（exit 1，与 argument 错误区分）；本里程碑保持 CLI 错误面不变，留 TODO 在 `enable.rs` 里指向 P1-G。

约束：

- 只放行 `agent-observability`；其它 capability 仍 `NOT_IMPLEMENTED`，hint 指向 supported capability。
- 只支持 `single_binary` / `binary` 与 `tar_gz` artifact（由 `InstallRunner` 决定），其它 backend 落到 `UnsupportedArtifactType`。
- 只支持 `file://` URL（由 `DownloadCache` 决定），其它 scheme 落到 `UnsupportedScheme`。
- 拒绝 `Blocked` plan；允许 `Degraded`（视为可执行，spec 与 Sub-C 一致）。
- 仅写 ANOLISA-owned roots（`bin_dir` / `etc_dir` / `state_dir` / `lib_dir` / `libexec_dir` / `datadir` / `log_dir` / `cache_dir`）。任何 dest 落到根外即 `ExternalPath`；外部文件不进 transaction、不进 backup，整个 milestone 不动。
- service enablement / systemd reload / health probe 全部不在范围内：写到 state 的 `ServiceRef.enabled = false`，留给后续命令处理。

`InstalledState` 写入：

- `kind=Capability` 对象：`name=plan.capability`、`version=plan.stability`（capability 无独立版本字段）、`status=Installed`、`component_refs=plan.components[].name`、`last_operation_id=<id>`。
- `kind=Component` 对象：`name=c.name`、`version=c.manifest_version`、`status=Installed`、`files=[OwnedFile { path, Anolisa, sha256 }]`、`services=[ServiceRef { name, manager, restartable=true, enabled=false }]`、`distribution_source=c.artifact.url`、`last_operation_id=<id>`。
- `OperationRecord { id, command="enable <cap>", status="ok", started_at, finished_at }`。

`CentralLog` 写入：

- 同一 `operation_id` 下追加 `started`（`status=null`，`severity=info`）和 `succeeded` (`status=ok`，`severity=info`，warnings 复制自 plan) 两条记录；失败路径用 `failed`（`status=failed`，`severity=error`）替换 `succeeded`。
- 查询：`anolisa logs --operation-id <id>` / `--operation-id <id> --json` 即可看到这两条；空 log 文件不视为错误。

限制：

- 不做 transaction / backup / rollback：若 `state.save` 之后 / `succeeded` 写入失败，cleanup 仍会 unlink ANOLISA-owned 文件并追加 `failed` 日志，但已经持久化的 state 文件不会撤销（属于已知的 P1-F 边界）。
- DownloadCache 当前只接 `file://`；HTTPS / signature verification 在 P1-G。
- InstallRunner 当前只接 `binary` / `tar_gz`；rpm / deb / oci / file 等 backend 后续里程碑。

## 当前未实现命令

以下命令已公开命令面，但仍应返回 `NOT_IMPLEMENTED` 或只允许后续 dry-run plan：

- `enable`（`agent-observability` 单 capability + 无 `--feature` / `--with-adapter` / `--from-source` 时 dry-run 与真实执行都已落地；其余 capability / flag 组合仍 `NOT_IMPLEMENTED`）
- `disable`
- `doctor`
- `restart`
- `info`
- `update`
- `subscription`
- `adapter`
- `self`
- `runtime install/remove/update/build/list/status`
- `osbase kernel/sandbox/security`

其中 `doctor --dry-run --fix` 已明确是无效组合，返回 `INVALID_ARGUMENT`。

## 验证状态

当前验证基线：

```bash
cd src/anolisa
cargo build --workspace
cargo test --workspace
cargo run -- env --json
cargo run -- logs --json
cargo run -- enable agent-observability --dry-run --json
```

已确认：

- `cargo test --workspace`：124 tests passed（CLI 28 + core 80 + capability_manifest 1 + env 6 + platform 9）。
- `cargo fmt --all -- --check`：通过。
- `anolisa env --json`：返回 `ok: true`。
- `anolisa logs --json`：fresh install 返回 `ok: true, data: []`。
- `anolisa list --json` / `anolisa status [CAPABILITY] --json`：返回 envelope，未安装条目带 `status: not_installed`。
- `anolisa enable agent-observability --dry-run --json`：在 macOS host 上返回 `ok: true, data.status: "blocked"`（precheck `os` 失败：expected linux, actual macos），无 panic、不写任何文件。
- `anolisa --install-mode system --prefix <tmp> enable agent-observability --json`（Smoke A，macOS host）：返回 `ok: false`、`error.code = INVALID_ARGUMENT`、reason 同时包含 `blocked` 与 `--dry-run`、exit code 2，且 `<tmp>/var/lib/anolisa/installed.toml` 与 `<tmp>/var/log/anolisa/central.jsonl` 均未创建（Sub-C 规定 Blocked plan 在 lock / log 之前拒绝）。Linux happy-path Smoke B 在 macOS dev host 上无法直接执行，留待 CI / Linux env 跑：预期 `ok: true, data.operation_id` 非空，`<tmp>` 下出现 `installed.toml`（含 capability + agentsight 对象、`OperationRecord.status="ok"`）+ `central.jsonl`（2 行，同 operation_id，第二行 `status=ok`），`anolisa logs --operation-id <id> --json` 能查询到 started + succeeded。

## enable 是否是最高优先级

结论：如果目标是让用户使用最基础功能，`anolisa enable <capability>` 是最高优先级的产品闭环；但实现上不能直接跳到真实安装执行，必须拆成两个阶段。

### 为什么 enable 是基础闭环

老板设计文档中的核心用户路径是 capability-first：

```text
env probe -> manifest load -> capability list -> enable dry-run plan -> enable execute -> status/logs
```

对用户来说，`enable agent-observability` 才代表 ANOLISA 真正产生价值：

- 自动检查环境依赖。
- 解析 capability 到组件。
- 选择预编译 artifact。
- 下载并校验二进制或 rpm/deb/tar_gz。
- 执行安装。
- 写入 `InstalledState`。
- 写入 `CentralLog`。
- 支持后续 `status`、`logs`、`disable`、`rollback`。

AgentSight / `agent-observability` 是 P0，因此第一条真实闭环应优先围绕：

```bash
anolisa enable agent-observability --dry-run
anolisa enable agent-observability
anolisa status agent-observability
anolisa logs agent-observability
```

### 为什么不能直接先做真实 enable

真实 enable 是第一个有副作用的命令，会改机器状态。它至少依赖以下基础设施：

| 阶段 | 依赖 | 当前状态 |
|---|---|---|
| env probe | `anolisa-env` | 最小可用，仍需补 systemd/eBPF/package manager probe |
| catalog load | `anolisa-core::Catalog` | 库可用，CLI `list` / `status` 已接入 |
| capability resolve | `CapabilityService` / planner | dry-run planner + 真实 executor 均已接入（P1-E2/P1-F） |
| distribution resolve | `DistributionIndex` resolver | 库可用，P1-F review fixup 后缺 sha256 直接 `Blocked`，不再 `Degraded` |
| artifact fetch | downloader/cache/checksum/signature | P1-F：`DownloadCache`（`file://` + sha256 streaming verify）；HTTPS / signature 留待 P1-G |
| install runner | rpm/deb/tar_gz/binary/file backend | P1-F：`InstallRunner`（`binary` + `tar_gz`）；ANOLISA-owned roots only；review fixup 后 fresh-install only（拒绝已存在目标），rpm/deb/file 留待 P1-G |
| lock | `InstallLock` | 库可用，P1-F 端到端使用，contention → `LockHeld` |
| backup | backup copy/restore | 当前仍 plan-only；ANOLISA-owned 文件 P1-F 通过 fresh-install 守卫规避冲突，真正 backup/restore 留待 P1-G |
| transaction | apply/verify/rollback boundary | P1-F：cleanup 段 = unlink ANOLISA-owned files + 恢复 `installed.toml` snapshot；完整 transaction boundary 留待 P1-G |
| state | `InstalledState v1` | schema 可用；P1-F 真实写入 + cleanup snapshot/restore |
| audit | `CentralLog` | query 可用；P1-F 接入 operation 三阶段记录（started/succeeded/failed）；per-phase（download/install/state-write）的细粒度记录留待 P1-G |

因此，真实 enable 的前置里程碑 `enable --dry-run` 已完成；P1-F 把上面列的 artifact fetch / install runner / state / audit / lock 全部接到了 `agent-observability` 一条最窄路径上。

## 下一步优先级

### P1-E1：只读接线（已完成）

目标：让用户能看到 ANOLISA 认识哪些 capability、当前机器装了什么。

- ✅ `list` 读取分层 `Catalog`（bundled + overlay）。
- ⏳ `list --available` 基于 `EnvFacts` 做最小可用性判断 —— 当前 stub 恒为 true，等 capability resolver / env probing 落地。
- ✅ `list --enabled` 读取 `InstalledState v1`，按 wire status 排除 `disabled` / `failed`。
- ✅ `status [CAPABILITY]` 读取 `InstalledState v1`，直接投出 `component_refs` / `enabled_features` / `health`。
- ✅ `status` 无 state 文件时返回空状态，不报错。

### P1-E2：enable dry-run plan（已完成）

目标：让 `enable` 第一次变成有真实业务语义但无副作用。

已落地：

- ✅ `anolisa enable agent-observability --dry-run` / `--dry-run --json`。
- ✅ `anolisa_core::plan_enable` 新增 `EnablePlan` / `ComponentPlan` / `ArtifactPlan` / `PrecheckResult` / `LayoutSummary` / `EnvFactsSummary` / `PlanStatus` / `PlanError`，纯函数、零 IO。
- ✅ Plan 暴露 capability 名称 + stability + install_mode、component 列表 + 命中的 artifact（type / backend / version / url / sha256）、env facts、prechecks、layout 摘要、warnings、next_actions、resolved_files。
- ✅ Precheck 同时评估 capability **与** component 两层 env_requirements（`kernel_min` / `btf` / `cap_bpf` / `libc` / `pkg_base`）；component 检查带命名空间，`unknown` 探针一律 warn 而非默认 ok。
- ✅ 状态聚合：fail / 任一 component blocked → `blocked`；warn / 版本漂移 / 空 index / component degraded → `degraded`；否则 `ready`。`blocked` 仍以 exit 0 + envelope 返回。（P1-F review fixup：缺 sha256 升格为 `blocked`，详见下文。）
- ✅ CLI scope 守卫：仅 `agent-observability` 放行；多 capability → `INVALID_ARGUMENT`；`--feature` / `--with-adapter` / `--from-source` / 非 `--dry-run` → 显式 `NOT_IMPLEMENTED`。
- ✅ 缺失 distribution index 不再致命：用空 index 出 plan、顶层 warning、组件 `blocked`。
- ✅ DistributionIndex 查找按 overlay → packaged → dev-tree（与 Catalog 分层一致）。
- ✅ Smoke：macOS / aarch64 host 上输出结构完整的 `blocked` plan（precheck `os` fail + `agentsight.os` fail + `agentsight.btf` warn），不写文件，不 panic。

### P1-F：enable execute 最小闭环（已完成）

目标：只打开最窄的真实执行路径，不做泛化。

建议第一条路径：

```text
agent-observability -> agentsight -> prebuilt tar_gz 或 rpm -> user-mode install
```

范围限制（落地版）：

- ✅ 只支持一个 P0 capability（`agent-observability`），CLI scope 守卫强制。
- ✅ 只支持预编译 artifact；`--from-source` 显式 `NOT_IMPLEMENTED`。
- ✅ 不支持 adapter 自动改第三方配置；`--with-adapter` 显式 `NOT_IMPLEMENTED`。
- ✅ 不支持 source build。
- ✅ 不支持复杂 rollback，只安装 ANOLISA-owned files；external file 修改继续禁止（`InstallRunner` 抛 `ExternalPath`）。
- ✅ 成功后写 `InstalledState v1`（capability + 各 component 对象 + `OperationRecord`）与 `CentralLog`（started + succeeded）。
- ✅ `InstallLock` 端到端使用：整个 execute 段在锁内，contention 直接 `LockHeld` 拒绝。
- ✅ 下载器（Sub-A，`DownloadCache`，仅 `file://` + sha256）、install runner（Sub-B，`InstallRunner`，仅 `binary` / `tar_gz`）、orchestrator（Sub-C，`enable_execute`，cleanup + failed log）、CLI wiring（Sub-D，本文档对应 commit）全部 green。

#### P1-F review fixup（已完成）

P1-F happy path 接起来后，code review 指出三个收紧项，已一并落地：

- ✅ **缺 sha256 升格为 `Blocked`**：`plan_enable` 中 `entry.sha256.is_none()` 不再产生 `Degraded` + warning，而是直接返回 `ComponentPlan { status: Blocked, blocked_reason: "...refuse to install without verification" }`。原因：`DownloadCache::fetch(url, None)` 不强制校验，`execute_enable` 又只拒绝 `Blocked` plan —— 旧路径会让无 sha256 的 artifact 真实安装。
- ✅ **state 写入 snapshot/restore**：`execute_enable` 在 `state.save()` 之前抓 `installed.toml` 字节快照；`state.save()` 或之后的 succeeded-log append 任意一步失败，cleanup 会把 `installed.toml` 还原为 pre-op 字节（无 prior 则删除）。修补此前 succeeded-log 失败时 cleanup 删 ANOLISA-owned files 但 `installed.toml` 仍声称"已安装"的不一致状态。
- ✅ **fresh-install only**：`InstallRunner::install` 在 `validate_dest` 之后再扫一遍 `dest.exists()`，命中即 `InstallError::DestExists`。P1-F 不做 backup/restore，因此覆盖已有 ANOLISA-owned 文件直接拒绝；真正的 backup + 升级路径留待 P1-G。

回归测试：planner `missing_sha256_marks_component_blocked`；executor `state_save_failure_restores_prior_installed_toml` + `state_save_failure_no_prior_state_leaves_no_installed_toml`；install runner `binary_install_refuses_to_overwrite_existing_dest` + `tar_gz_install_refuses_when_any_dest_preexists`。

### P1-G：execute 路径补齐与对称命令

P1-F 落地后还差几个明确缺口，按落地难度排：

- `CliError::Runtime { command, reason }` + `EXECUTION_FAILED` 错误码（exit 1）：把 download / install / state / log / lock 等 runtime 失败从 `INVALID_ARGUMENT`（exit 2）里独立出来，便于上游脚本区分参数错误 vs 真实执行错误。
- DownloadCache 加 HTTPS scheme + retry / progress：当前 `file://` only，覆盖 GitHub Release 之类的真实 artifact 源。
- Signature verification：在 sha256 之外要求 detached signature（首版可走 minisign / cosign），与 distribution-index 已有的 `signature` 字段对齐。
- 扩 artifact backend：rpm / deb（走系统包管理器、保留 transaction 概念）、oci（image pull）、tar_xz 等。
- Per-phase CentralLog 记录：当前 `execute_enable` 只写 operation 级 `started` / `succeeded` / `failed`；P1-G 需补 download 起停、install 起停、state-write 起停的 phase 级记录，方便 `anolisa logs` 定位具体失败阶段。
- ANOLISA-owned 文件 backup/restore：P1-F 通过 `InstallError::DestExists` 守住 fresh-install；P1-G 把 `backup.rs` 从 plan-only 升级到 copy/restore，让 reinstall / upgrade 能在覆盖前 backup、失败时 restore。
- `disable` 对称路径：用同套 `InstallLock` + `CentralLog` 脚手架解绑 state objects，并 unlink ANOLISA-owned files；不在 Sub-D 范围内但库层（state.upsert_object / state.remove_object）已就绪。
- 首条 external-file adapter case：要求 transaction / backup 真接，以一个 well-defined adapter（如 sshd_config 改一行）为目标。
- `update` / `uninstall`：在 `disable` 落地后基于 InstalledState diff 渐进打开。

## 待决策问题

- AgentSight 第一版 artifact 形态优先用 `tar_gz` 还是 rpm。
- GitHub Release index 是否作为默认 DistributionIndex 来源。
- `enable` 首版是否只允许 `--install-mode user`，system mode 延后。
- checksum 必须先强制 sha256，签名校验是否首版 hard requirement。
- `status` 的 health probe 首版是否只检查 binary/service/state，eBPF runtime health 是否后置。

## 后续动作

- ✅ `list` 和 `status` 只读 CLI wiring（P1-E1 已完成）。
- ✅ `enable agent-observability --dry-run` 接入 `plan_enable`（P1-E2 已完成）。
- ✅ `enable agent-observability` 最小真实安装闭环（P1-F 已完成，含 review fixup）：`DownloadCache` (`file://` + sha256) → `InstallRunner` (`binary` / `tar_gz`，仅 ANOLISA-owned roots，fresh-install only) → `InstalledState v1` capability + component upsert（cleanup 走 snapshot/restore）→ `CentralLog` started/succeeded/failed → `InstallLock` 整段持有 → 中途失败自清理 + 缺 sha256 提前 `Blocked`。
- 下一步是 P1-G（上面列出的方向：tighter exit code、网络下载、签名校验、per-phase audit 记录、ANOLISA-owned 文件 backup/restore、disable / uninstall 对称路径）。
- 其它 mutating 命令（`disable` / `restart` / `update` / `uninstall` / `subscription` / `adapter` / `self` / `runtime *` / `osbase *` 等）在 P1-G 之前继续返回 `NOT_IMPLEMENTED`。
