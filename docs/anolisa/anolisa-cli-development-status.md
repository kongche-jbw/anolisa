# ANOLISA CLI Development Status

本文记录 `anolisa` CLI 当前开发状态、可用功能边界和下一步优先级。更新基准：

- 日期：2026-06-01
- 分支：`kongche/dev/anolisa-p1`
- HEAD：P1-E2 完成 + fixup（component env prechecks、missing index 不再致命、overlay 优先、resolved_files 渲染）
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
- 所有真实有副作用命令（`enable`（非 dry-run）/`disable`/`restart`/`update`/...）仍返回 `NOT_IMPLEMENTED`。
- `backup.rs` 仍是 plan-only；因此真实 `enable/install/uninstall/update/restart/rollback` 不能打开执行路径。

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
```

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

## 当前未实现命令

以下命令已公开命令面，但仍应返回 `NOT_IMPLEMENTED` 或只允许后续 dry-run plan：

- `enable`（仅 `agent-observability --dry-run` 已接入 planner，其余形态仍 `NOT_IMPLEMENTED`）
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

- `cargo test --workspace`：99 tests passed（CLI 26 + core 57 + capability_manifest 1 + env 6 + platform 9）。
- `cargo fmt --all -- --check`：通过。
- `anolisa env --json`：返回 `ok: true`。
- `anolisa logs --json`：fresh install 返回 `ok: true, data: []`。
- `anolisa list --json` / `anolisa status [CAPABILITY] --json`：返回 envelope，未安装条目带 `status: not_installed`。
- `anolisa enable agent-observability --dry-run --json`：在 macOS host 上返回 `ok: true, data.status: "blocked"`（precheck `os` 失败：expected linux, actual macos），无 panic、不写任何文件。

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
| catalog load | `anolisa-core::Catalog` | 库可用，CLI `list` 已接入 |
| capability resolve | `CapabilityService` / planner | dry-run planner 已接入；真实执行 service 未实现 |
| distribution resolve | `DistributionIndex` resolver | 库可用 |
| artifact fetch | downloader/cache/checksum/signature | 未实现 |
| install runner | rpm/deb/tar_gz/binary/file backend | 未实现 |
| lock | `InstallLock` | 库可用 |
| backup | backup copy/restore | 当前 plan-only |
| transaction | apply/verify/rollback boundary | 未实现 |
| state | `InstalledState v1` | schema 可用 |
| audit | `CentralLog` | query 可用，operation write 需接入执行链路 |

因此，真实 enable 的前置里程碑 `enable --dry-run` 已完成；下一步才进入 downloader / install runner / state + audit 写入。

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
- ✅ 状态聚合：fail / 任一 component blocked → `blocked`；warn / 版本漂移 / 缺 sha256 / 空 index / component degraded → `degraded`；否则 `ready`。`blocked` 仍以 exit 0 + envelope 返回。
- ✅ CLI scope 守卫：仅 `agent-observability` 放行；多 capability → `INVALID_ARGUMENT`；`--feature` / `--with-adapter` / `--from-source` / 非 `--dry-run` → 显式 `NOT_IMPLEMENTED`。
- ✅ 缺失 distribution index 不再致命：用空 index 出 plan、顶层 warning、组件 `blocked`。
- ✅ DistributionIndex 查找按 overlay → packaged → dev-tree（与 Catalog 分层一致）。
- ✅ Smoke：macOS / aarch64 host 上输出结构完整的 `blocked` plan（precheck `os` fail + `agentsight.os` fail + `agentsight.btf` warn），不写文件，不 panic。

### P1-F：enable execute 最小闭环

目标：只打开最窄的真实执行路径，不做泛化。

建议第一条路径：

```text
agent-observability -> agentsight -> prebuilt tar_gz 或 rpm -> user-mode install
```

范围限制：

- 先只支持一个 P0 capability。
- 先只支持预编译 artifact。
- 先不支持 adapter 自动修改第三方 agent 配置。
- 先不支持 source build。
- 先不支持复杂 rollback，只允许安装 ANOLISA-owned files；任何 external file 修改继续禁止。
- 成功后必须写 `InstalledState` 和 `CentralLog`。

## 待决策问题

- AgentSight 第一版 artifact 形态优先用 `tar_gz` 还是 rpm。
- GitHub Release index 是否作为默认 DistributionIndex 来源。
- `enable` 首版是否只允许 `--install-mode user`，system mode 延后。
- checksum 必须先强制 sha256，签名校验是否首版 hard requirement。
- `status` 的 health probe 首版是否只检查 binary/service/state，eBPF runtime health 是否后置。

## 后续动作

- ✅ `list` 和 `status` 只读 CLI wiring（P1-E1 已完成）。
- ✅ `enable agent-observability --dry-run` 接入 `plan_enable`（P1-E2 已完成）。
- 下一步：打开 `enable agent-observability` 的最小真实安装闭环（P1-F）。需要按依赖序解锁：downloader + checksum → install runner（先 tar_gz / 单 binary）→ `InstalledState` 写入 → `CentralLog` operation 写入 → 最窄 backup/rollback（仅 ANOLISA-owned files）→ `InstallLock` 端到端使用。
- 所有 mutating 命令在 transaction/backup/rollback 未完成前继续返回 `NOT_IMPLEMENTED`。
