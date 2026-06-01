# ANOLISA CLI Development Status

本文记录 `anolisa` CLI 当前开发状态、可用功能边界和下一步优先级。更新基准：

- 日期：2026-06-01
- 分支：`kongche/dev/anolisa-p1`
- HEAD：`6f1b4c8 merge: P1-fixup-5 operation_id filter + logs CLI wiring`
- 定位：P1 可开发骨架；不是可安装组件的产品形态

## 当前结论

- CLI 命令面、全局参数、JSON envelope、`NOT_IMPLEMENTED` / `INVALID_ARGUMENT` 错误码已经稳定。
- 已落地的基础库包括 `EnvService`、`FsLayout`、`Catalog`、`DistributionIndex` resolver、`InstalledState v1`、`CentralLog`、install lock、backup plan skeleton。
- 当前可真实使用的命令主要是 `env` 和 `logs`。
- `list` 目前是 placeholder 成功输出，不能当真实 capability catalog 使用。
- `status` 和所有有副作用命令仍未实现。
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

状态：placeholder。

当前行为：

```json
{
  "filter": "all",
  "note": "Capability Resolver not yet wired"
}
```

限制：

- 尚未读取 `Catalog`。
- 尚未叠加 `InstalledState`。
- 尚未根据 `EnvFacts` 判断 available/degraded/unavailable。

## 当前未实现命令

以下命令已公开命令面，但仍应返回 `NOT_IMPLEMENTED` 或只允许后续 dry-run plan：

- `enable`
- `disable`
- `status`
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
```

已确认：

- `cargo test --workspace`：63 tests passed。
- `anolisa env --json`：返回 `ok: true`。
- `anolisa logs --json`：fresh install 返回 `ok: true, data: []`。

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
| catalog load | `anolisa-core::Catalog` | 库可用，CLI `list` 未接 |
| capability resolve | `CapabilityService` / planner | 未实现 |
| distribution resolve | `DistributionIndex` resolver | 库可用 |
| artifact fetch | downloader/cache/checksum/signature | 未实现 |
| install runner | rpm/deb/tar_gz/binary/file backend | 未实现 |
| lock | `InstallLock` | 库可用 |
| backup | backup copy/restore | 当前 plan-only |
| transaction | apply/verify/rollback boundary | 未实现 |
| state | `InstalledState v1` | schema 可用 |
| audit | `CentralLog` | query 可用，operation write 需接入执行链路 |

因此，真实 enable 的前置里程碑应该是 `enable --dry-run`，先把 plan 生成可信。

## 下一步优先级

### P1-E1：只读接线

目标：让用户能看到 ANOLISA 认识哪些 capability、当前机器装了什么。

- `list` 读取 `Catalog`。
- `list --available` 基于 `EnvFacts` 做最小可用性判断。
- `list --enabled` 读取 `InstalledState v1`。
- `status [CAPABILITY]` 读取 `InstalledState v1`。
- `status` 无 state 文件时返回空状态，不报错。

### P1-E2：enable dry-run plan

目标：让 `enable` 第一次变成有真实业务语义但无副作用。

优先实现：

```bash
anolisa enable agent-observability --dry-run
anolisa enable agent-observability --dry-run --json
```

dry-run plan 必须展示：

- capability 名称和优先级。
- 解析出的 component 列表。
- 环境检查结果。
- 匹配到的 artifact。
- install mode 和路径布局。
- 将要修改/写入的文件。
- 需要的权限。
- 不满足条件时的降级原因和修复建议。

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

- 先接 `list` 和 `status` 只读 CLI wiring。
- 再接 `enable agent-observability --dry-run`。
- 最后打开 `enable agent-observability` 的最小真实安装闭环。
- 所有 mutating 命令在 transaction/backup/rollback 未完成前继续返回 `NOT_IMPLEMENTED`。
