# anolisa-core Spec

对应 launch spec：S3 Manifest Schema Spec、S5 Catalog and Resolver Spec、
S6 Planner Spec、S8 State and Audit Spec。

## 当前结论

- `anolisa-core` 是 capability/component/feature/distribution/state 的业务核心。
- CLI 不硬编码 OpenClaw/Hermes、AgentSight setcap、RPM 包名、构建命令。
- 写系统的操作必须走 plan、transaction、state、central log、rollback boundary。

## 核心对象

```text
Catalog
Capability
Component
Feature
TargetProvider
DistributionIndex
Plan
PlanStep
Transaction
InstalledState
CentralLog
Backup
Rollback
HealthCheck
```

## Service 边界

| Service | 责任 |
|---|---|
| CatalogService | 加载 capability/component/target provider/distribution index |
| CapabilityService | `list/enable/disable/status/doctor` 的业务语义 |
| RuntimeService | `runtime install/remove/update/build/list/status` |
| DistributionResolver | 根据 manifest selector + DistributionIndex 选择 artifact |
| Planner | 生成 dry-run plan 和可执行 plan |
| TransactionManager | 执行有副作用的步骤并记录 rollback boundary |
| StateStore | 读写 installed state |
| CentralLogStore | 写入 operation/audit 和 component reported logs |
| BackupStore | 外部配置修改前后的 backup metadata |

## Planner 流程

所有有副作用的命令遵循：

```text
resolve -> precheck -> plan -> backup -> transaction -> verify -> state -> audit
```

要求：

- `--dry-run` 只走到 plan，不写文件、不改服务、不写 state。
- 修改第三方 agent 配置前必须创建 ANOLISA 侧 backup。
- rollback 只删除 ANOLISA-owned files；external file 只能从 backup 恢复。
- `doctor --fix` 是显式执行修复，不再额外交互确认。

## State 最小字段

参考 `../../templates/installed-state.toml`。

`InstalledState` 至少记录：

- capability/component/adapter/osbase object。
- version、manifest digest、distribution source。
- install mode、prefix、installed files。
- owned files 与 external modified files。
- services、systemd units、socket、config。
- backup ids。
- last health status。
- subscription/adopt 管理状态。

## Central Log

`logs` 的数据源由 `CentralLogStore` 管理：

- ANOLISA 自身 operation/audit logs。
- 已纳管组件主动上报的组件日志。

`logs xxx` 是过滤语义，`xxx` 可以是 capability、component、operation id、log source 或 `all`。

## 模板

- `../../templates/capability.toml`
- `../../templates/component-runtime.toml`
- `../../templates/component-osbase.toml`
- `../../templates/distribution-index.toml`
- `../../templates/installed-state.toml`
