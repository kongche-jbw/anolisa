# ANOLISA CLI Launch Spec Breakdown

本文用于把 `anolisa` CLI 从当前 skeleton 推进到“可以上线、可以并行开发”的状态。

前提决策：

- 默认输出第一优先级是人类可读，`--json` 是可选机器可读输出。
- 官网安装脚本只安装 `anolisa` CLI 预编译二进制。
- 组件默认使用预编译产物，源码构建只用于开发或显式 `--from-source`。
- 组件 manifest 只声明安装选择策略；具体 artifact URL、checksum、signature、backend 统一进入 DistributionIndex。
- GitHub Release RPM 可以作为早期 `type=rpm, backend=github-release`，但不是唯一分发标准。
- 本文先抛开 `build-all.sh` 和旧 adapter runner，不把它们作为产品主路径。
- CLI 需要保持强扩展性和松耦合：新增 capability、component、target provider 时，不应该修改中心命令逻辑。

## 1. 什么叫可以上线开发

“可以上线开发”不是所有组件都生产可用，而是 CLI 的框架契约稳定，其他同学可以按 spec 并行补实现。

最低标准：

| 项 | 要求 |
|---|---|
| Command surface | `anolisa --help` 中公开的命令语义明确；未实现命令可以公开，但必须稳定返回 `not implemented` |
| Human output | 默认输出面向人类，短、清楚、有修复建议；不能只输出 JSON |
| JSON output | `--json` 使用同一 response model，不单独实现一套逻辑 |
| Manifest | capability/component/target provider/distribution selector schema 可验证 |
| Catalog | 能加载内置 manifest，并能 lint 跨引用关系 |
| Env | `anolisa env` 输出真实探测结果，不再 placeholder |
| Planner | `enable --dry-run` 可以生成可信 plan，不修改系统 |
| State | 未安装、已安装、部分安装、降级状态有统一模型 |
| Extensibility | 新组件或新 target provider 不需要改 `commands/*.rs` 的业务分支 |
| Tests | command snapshot、manifest schema、planner dry-run 有测试 |

## 2. Spec 拆分

建议拆成以下 10 份 spec。每份 spec 都应该能独立评审、独立开发。

| Spec | 负责人 | 解决什么问题 | 产物 |
|---|---|---|---|
| S1 Command Surface Spec | CLI owner | 哪些命令稳定、实验、暂未实现；每个命令语义和副作用 | command table、help 文案、待决策表 |
| S2 UX and Output Spec | CLI + 产品 | 人类可读输出、错误文案、`--json` envelope、exit code | formatter trait、snapshot cases |
| S3 Manifest Schema Spec | core owner | capability/component/target provider/distribution 字段 | TOML schema、lint rules、examples |
| S4 Env Probe Spec | env owner | 机器事实如何探测、缓存、降级 | `EnvFacts` schema、probe list、mock fixture |
| S5 Catalog and Resolver Spec | core owner | capability 如何映射 component/features/backends | resolver API、availability model |
| S6 Planner Spec | core + platform | enable/update/disable 如何形成 dry-run plan | `PlanStep` schema、phase model |
| S7 Distribution Spec | release + platform | 官网安装脚本、DistributionIndex、组件预编译产物、校验和签名 | index schema、asset naming、checksum/signature、backend matrix |
| S8 State and Audit Spec | core + security | installed state、owned files、rollback/audit | state schema、lock policy、audit schema |
| S9 Target Provider Spec | adapter/platform owner | OpenClaw/Hermes/Codex/Claude Code 等平台差异如何抽象 | provider trait、mapping rules |
| S10 AgentSight P0 Spec | AgentSight owner | 商业化观测入口的 host/guest/container/rootless 策略 | observation mode matrix、doctor checks |

当前建议优先级：

```text
Week 1: S1 + S2 + S3 + S4 + S5 + S7
Week 2: S6 + S8 + S9 + S10
```

原因：命令面、输出、manifest、env、resolver 和 distribution 不定，后续开发会互相打架。

## 3. 统一 CommandSpec 模板

每个命令都要补一份 `CommandSpec`，字段如下：

```text
command:
  name:
  layer: tier1-capability | tier2-runtime | tier2-osbase | target-provider | self
  audience: human | developer | operator | commercial
  stability: stable | experimental | hidden
  purpose:
  inputs:
  default_human_output:
  json_output:
  side_effects: none | state-write | filesystem-write | service-change | network
  dry_run_behavior:
  idempotency:
  state_used:
  manifest_used:
  exit_codes:
  examples:
  open_questions:
```

上线前规则：

- `stability=stable` 的命令必须行为明确、输出稳定、有测试。
- `experimental` 可以出现在 `--help`，但 help 文案必须写清楚边界。
- 已公开但未实现的命令必须返回 `NOT_IMPLEMENTED`，不能 panic、不能静默成功。
- `hidden` 不出现在默认 `--help`，但代码可以先保留。
- 有写系统副作用的命令必须支持 `--dry-run`。

## 4. 输出规范

默认输出是人类可读。

示例：

```text
$ anolisa env
Platform     Container (runc)
OS           Alibaba Cloud Linux 4
Kernel       6.6.30  BTF: yes  cgroup v2: yes
Arch         x86_64
Privilege    rootless  CAP_BPF: no  CAP_PERFMON: no
Storage      btrfs: no  overlayfs: yes
Targets      openclaw: found  hermes: not found

AgentSight   unavailable
Reason       CAP_BPF is not available in this container.
Advice       Install on the host, or rerun in a privileged container.
```

`--json` 是可选项：

```json
{
  "ok": true,
  "schema_version": 1,
  "command": "env",
  "data": {},
  "warnings": []
}
```

约束：

- human output 和 JSON output 必须来自同一 response struct。
- `--json` 时 stdout 只允许 JSON；human 日志和调试信息进 stderr。
- `--quiet` 对 human output 生效；错误仍输出。
- `--verbose` 增加细节，不改变语义。

## 5. 松耦合架构约定

中心 CLI 只认识抽象对象：

```text
Capability
Component
Feature
Backend
TargetProvider
Artifact
PlanStep
InstalledState
HealthCheck
```

中心 CLI 不应该硬编码：

- OpenClaw/Hermes/Codex/Claude Code 的 home 目录和插件目录。
- Tokenless/ws-ckpt/Sec-Core 的构建命令。
- AgentSight 的 setcap 细节。
- sandbox 后端具体安装命令。
- RPM/DEB/npm 包名推断规则。

这些应该分别放在：

| 信息 | 所属位置 |
|---|---|
| 用户视角能力 | capability manifest |
| 组件版本、安装模式、artifact selector、依赖 | component manifest |
| 平台目录、hook 映射、restart hint | target provider |
| 预编译产物 URL、包名、checksum、signature、backend | DistributionIndex |
| 环境事实 | env probe |
| 安装状态 | installed state |

## 6. 对照老板设计文档的对齐结论

老板文档中的主线仍然成立：

- `anolisa` 是 Control Plane，面向人类 operator/developer/evaluator。
- 日常入口仍然是 capability-first：`env/list/enable/status/doctor`。
- Tier 2 surface 仍然公开：`subscription/adapter/self/runtime/osbase`。
- `adapter` 继续作为对外命令名；内部实现抽象为 `TargetProvider`。
- `--json` 是可选机器输出，默认输出优先人类可读。

本 spec 对老板设计文档做了几个收敛：

| 主题 | 老板设计文档 | 本 spec 决策 |
|---|---|---|
| `logs` | 查看 capability service logs | 改为中心化日志入口：包含 `anolisa` operation/audit logs 和组件主动上报日志，可按 capability/component/operation/log source 过滤 |
| `restart` | 重启 capability underlying service | 只重启已安装、由 ANOLISA 管理、声明 restartable 的服务 |
| `update` | `update [capability|all]`、`self update`、`runtime update` 并存 | 收敛为一个 update surface：`update self`、`update runtime <component|all>`、`update all` |
| `enable --feature` | feature 参数属于 capability verb | 保留；先 enable capability，再 enable 指定 feature |
| `doctor --fix` | `doctor [capability] [--fix]` | 无参数只读检查状态；`--fix` 直接执行修复，但必须走 transaction/audit/backup |
| `subscription` | 公开商业化 surface | 公开并定义 spec；第一阶段可返回 `not implemented` |
| `self adopt` | build-all migration | 保留为“纳入 ANOLISA 管理和统计范围”，后续与 subscription 结合；第一阶段可返回 `not implemented` |
| `osbase` | 公开 osbase surface | 进入公开命令面；未实现部分返回 `not implemented` |

## 7. 最终命令语义

### 7.1 `logs`

`anolisa logs` 是 ANOLISA 的中心化日志查询入口，不是用户 workload 日志，也不是普通 agent 平台日志。

它包含两类日志：

- ANOLISA 自身 operation/audit logs。
- 已纳入 ANOLISA 管理的组件主动上报的组件日志。

记录范围：

- `enable/disable/update/runtime install/runtime remove/osbase install/adapter install/subscription/adopt` 等操作。
- 每次操作的 plan、执行步骤、结果、耗时、调用用户、install mode。
- 被修改的 ANOLISA-owned 文件列表。
- 对第三方 agent 配置文件的备份记录，例如 OpenClaw/Hermes/Codex 配置修改前后的 digest、backup path、rollback id。
- rollback、repair、doctor fix 的执行记录。
- 组件上报的状态事件、关键运行日志、健康检查摘要、商业化观测所需的最小事件数据。

日志路径由 `FsLayout` 决定：

```text
user-mode:   $XDG_STATE_HOME/anolisa/logs/operation.log
user-mode:   $XDG_STATE_HOME/anolisa/logs/component.log
system-mode: /var/log/anolisa/operation.log
system-mode: /var/log/anolisa/component.log
```

当前 `--help` 是：

```text
anolisa logs <CAPABILITY> [--follow] [--since] [--lines]
```

应调整为：

```text
anolisa logs [OBJECT] [--follow] [--since <DURATION>] [--lines <N>]
```

`OBJECT` 可以是 capability、component、operation id、log source 或 `all`。例如 `anolisa logs agentsight` 查询 AgentSight 相关日志，`anolisa logs op-20260601-001` 查询某次操作，`anolisa logs component` 查询组件上报日志。

开发归属：

- `anolisa-cli`: 参数解析、人类可读/JSON formatter。
- `anolisa-core`: `CentralLogStore`、operation log、component log ingest、query/filter、operation id。
- `anolisa-platform`: 日志路径、权限、文件轮转。

### 7.2 `restart`

`anolisa restart <CAPABILITY>` 只重启已安装且声明 restartable 的 ANOLISA-managed service。

约束：

- 不重启用户 agent runtime 本身。
- 不重启 OpenClaw/Hermes/Codex/Claude Code，除非它们是 ANOLISA-owned install。
- 如果 capability 映射到多个 service，必须先生成 restart plan。
- 如果 service 不可重启，返回 `NOT_RESTARTABLE`，并给出 advice。

开发归属：

- `anolisa-core`: capability -> component -> service resolution、restart plan。
- `anolisa-platform`: systemd/user service backend。
- `anolisa-cli`: human/JSON 输出。

### 7.3 `update`

更新语义收敛到一个 top-level update surface。

目标命令：

```text
anolisa update self
anolisa update runtime <component|all>
anolisa update all
```

语义：

- `update self`: 只更新 `anolisa` CLI 本体。
- `update runtime <component>`: 更新某个 runtime component，例如 `agentsight/tokenless/ws-ckpt`。
- `update runtime all`: 更新所有已纳入 ANOLISA 管理的 runtime components。
- `update all`: 更新所有已纳入 ANOLISA 管理的 runtime/osbase/adapters，不包含 `anolisa` CLI 本体；subscription credential/entitlement refresh 由 `subscription refresh` 管理。

当前 `--help` 中已有 `self update` 和 `runtime update`，实现阶段可以先作为兼容 alias，但长期 help 应向 `update self/runtime/all` 收敛。

已确认：`update all` 不包含 `self`。CLI 自更新必须使用独立命令 `anolisa update self`，避免 CLI 自更新和组件更新混在同一个 transaction。

### 7.4 `enable --feature`

保留 `--feature`，但语义必须固定：

```text
anolisa enable <capability> --feature <name>
```

执行逻辑：

1. 如果 capability 未启用，先启用 capability 的必需组件和基础默认配置。
2. 再启用指定 feature。
3. 如果 capability 已启用，只启用指定 feature。
4. 如果 feature 需要额外 env/backend/adapter，进入同一 planner。
5. 所有步骤都进入 state、audit、rollback 体系。

这和老板文档中的 feature toggle 一致，但更明确地允许“先启 capability，再启 feature”。

### 7.5 `doctor --fix`

`anolisa doctor` 无参数时是只读状态检查，行为类似 dry-run，不修改系统。

```text
anolisa doctor
anolisa doctor agent-observability
```

`--fix` 表示直接修复：

```text
anolisa doctor --fix
anolisa doctor agent-observability --fix
```

约束：

- `--fix` 必须先生成 fix plan，再执行。
- 任何写操作必须进入 transaction。
- 修改第三方 agent 配置前必须创建 ANOLISA 侧 backup。
- 修复完成后必须执行 health check。
- 所有修复必须写 central log 的 operation 记录。
- 显式传入 `--fix` 就代表执行修复，不再额外要求交互确认。
- `doctor --dry-run --fix` 是无效组合，应返回 `INVALID_ARGUMENT`。`doctor` 默认已经是只读检查；需要修复时只使用 `doctor --fix`。

### 7.6 `adapter`

对外继续叫 `adapter`。内部实现叫 `TargetProvider`。

语义：

- `adapter scan`: 探测可用 target provider，例如 OpenClaw/Hermes/Codex/Claude Code/Qwen Code。
- `adapter list`: 列出 component exports 和 target binding 状态。
- `adapter install <component> <target>`: 将某 component 的 plugin/skill/hook/tool/MCP export 绑定到某 target。
- `adapter remove <component> <target>`: 移除 ANOLISA-owned binding，并回滚 ANOLISA 修改过的目标配置。

### 7.7 `subscription`

公开并定义 spec，第一阶段可以返回 `not implemented`。

语义：

- `subscription register`: 注册当前机器或当前用户。
- `subscription status`: 查看订阅、entitlement、离线 token、统计上传状态。
- `subscription refresh`: 刷新 entitlement。
- `subscription unregister`: 注销本机或当前用户绑定。

与 adopt 的关系：

- adopt 将本机已有组件纳入 ANOLISA installed state。
- subscription 决定这些组件是否进入商业化统计、授权、上报范围。

### 7.8 `self adopt`

保留。第一阶段可以返回 `not implemented`，但 spec 必须明确。

语义：

- 扫描机器上已有 ANOLISA components。
- 将用户已经安装的组件纳入 `installed.toml` 管理。
- 记录 component version、source、files、manifest digest、health status。
- 后续与 subscription 结合，用于纳入统计管理范围。

不再把 adopt 绑定为 `build-all.sh migration path`。`build-all.sh` 只是可能的 source hint。

### 7.9 `runtime install --component-version`

接受把当前 `runtime install --version` 改名为：

```text
anolisa runtime install <component> --component-version <VERSION>
```

原因：

- 当前 `--version` 与 clap 全局 `--version` 冲突，会导致 `runtime install --help` panic。
- `--component-version` 语义更清晰。

上线前必须修复这个 panic。已公开命令不能 panic。

### 7.10 `osbase`

`osbase` 进入公开命令面。未实现部分返回 `not implemented`。

语义：

- `osbase kernel`: 管理 ANOLISA-optimized kernel/modules/eBPF base。
- `osbase sandbox`: 管理 container/kata/firecracker/vm/landlock 等 substrate。
- `osbase security`: 管理 loongshield/seccomp-profiles 等 security overlay。

约束：

- install/remove 都是高风险操作，必须支持 dry-run plan。
- 必须写 central log 的 operation 记录。
- 必须有 rollback boundary。
- 必须记录 owned files、service、kernel modules、config backup。

## 8. 类软件包管理机制

`anolisa` 要按软件包管理器的严谨程度管理 ANOLISA stack。

核心对象：

```text
Catalog
Manifest
DistributionIndex
Plan
Transaction
Backup
InstalledState
CentralLog
HealthCheck
Rollback
Lock
```

每次有副作用的操作必须遵循：

```text
resolve -> precheck -> plan -> backup -> transaction -> verify -> state -> audit
```

### 8.1 State

`InstalledState` 必须记录：

- capability/component/adapter/osbase object。
- version、manifest digest、distribution source。
- install mode、prefix、installed files。
- owned files 与 external modified files 分开记录。
- services、systemd units、socket、config。
- backup ids。
- last health status。
- subscription/adopt 管理状态。

### 8.2 Backup

当 adapter 或 osbase 修改外部配置时，必须创建 ANOLISA 侧 backup。

示例：

```text
~/.local/state/anolisa/backups/<operation-id>/openclaw/config.json
~/.local/state/anolisa/backups/<operation-id>/manifest.json
```

backup manifest 记录：

- original path。
- backup path。
- sha256 before/after。
- owner: external | anolisa。
- restore strategy。

### 8.3 Rollback

rollback 不只删除文件，还要恢复外部配置。

规则：

- 只删除 state 中 `owner=anolisa` 的文件。
- 对 external file 只恢复 backup，不直接删除。
- service restart/daemon-reload 也必须作为 rollback step。
- rollback 失败要进入 central log 和 doctor report。

### 8.4 Central Log

central log 是 `logs` 命令的数据来源，包含 ANOLISA operation/audit logs 和组件主动上报日志。

每条记录至少包含：

```json
{
  "kind": "operation|component",
  "operation_id": "op-20260601-001",
  "command": "enable agent-observability",
  "source": "anolisa-cli|agentsight|tokenless|sec-core|ws-ckpt",
  "component": "agentsight",
  "severity": "debug|info|warn|error",
  "message": "enable agent-observability finished",
  "actor": "kongche",
  "install_mode": "user",
  "started_at": "2026-06-01T10:00:00Z",
  "finished_at": "2026-06-01T10:00:03Z",
  "status": "ok|failed|rolled_back|partial",
  "objects": ["agent-observability", "agentsight"],
  "backup_ids": [],
  "warnings": []
}
```

### 8.5 Lock

所有写 state、install files、backup、central log 的操作必须持有 lock。

建议：

```text
user-mode:   $XDG_STATE_HOME/anolisa/lock
system-mode: /var/lib/anolisa/lock
```

## 9. 当前 `--help` 参数到 Rust 包的开发归属

### 9.1 Global options

| 参数 | 语义 | 开发归属 |
|---|---|---|
| `--install-mode <user|system>` | 安装作用域 | `anolisa-cli` 解析；`anolisa-platform::FsLayout` 解析路径；`anolisa-core::state/transaction` 使用 |
| `--prefix <PATH>` | system mode prefix override | `anolisa-cli` 解析；`anolisa-platform::FsLayout` 校验 |
| `--json` | 可选机器输出 | `anolisa-cli` formatter |
| `--dry-run` | 只生成 plan，不执行 | `anolisa-cli` context；`anolisa-core::planner/transaction` 强制只读 |
| `-v/--verbose` | 增加 human 输出细节 | `anolisa-cli` formatter；`anolisa-env` probe details |
| `-q/--quiet` | 抑制非错误 human 输出 | `anolisa-cli` formatter |
| `--no-color` | 禁用颜色 | `anolisa-cli` formatter |
| `-h/--help` | clap help | `anolisa-cli` |
| `-V/--version` | CLI 版本 | `anolisa-cli` |

### 9.2 Tier 1 capability commands

| 命令/参数 | Spec 语义 | 开发归属 |
|---|---|---|
| `list --available --enabled` | capability catalog + state 过滤 | `anolisa-cli::commands::tier1::list`; `anolisa-core::catalog/capability/state`; `anolisa-env` |
| `enable <CAPABILITIES>...` | 启用 capability 默认能力 | `anolisa-cli::commands::tier1::enable`; `anolisa-core::capability/planner/transaction`; `anolisa-env`; `anolisa-platform`; `anolisa-build` 仅 `--from-source` |
| `enable --feature <NAME>` | 先启 capability，再启指定 feature | `anolisa-core::feature_flags`; `anolisa-core::planner`; `anolisa-env::gate` |
| `enable --with-adapter <target|auto>` | 绑定 component export 到 target | `anolisa-core::target_provider`; `anolisa-env::probes::frameworks`; `anolisa-cli::commands::adapter` shared service |
| `enable --from-source` | 显式源码构建 | `anolisa-build`; `anolisa-core::planner`; 默认路径仍是 prebuilt distribution |
| `disable <CAPABILITY>` | 禁用 capability | `anolisa-core::planner/transaction/state`; `anolisa-platform` |
| `disable --feature <NAME>` | 关闭 feature，不卸载 capability | `anolisa-core::feature_flags/state` |
| `disable --purge` | 删除 owned files/config/state | `anolisa-core::transaction/rollback`; `anolisa-platform::fs_layout` |
| `status [CAPABILITY]` | state + health check | `anolisa-core::state/health`; `anolisa-platform::systemd`; `anolisa-cli` formatter |
| `doctor [CAPABILITY] --fix` | 无 fix 只读检查；`--fix` 执行修复 | `anolisa-core::doctor/fix_plan/transaction`; `anolisa-env`; `anolisa-platform` |
| `logs [OBJECT] --follow --since --lines` | 查询中心化日志：operation/audit + component reported logs | `anolisa-core::central_log`; `anolisa-platform` log path/rotation；`anolisa-cli` formatter |
| `restart <CAPABILITY>` | 重启 restartable ANOLISA-managed service | `anolisa-core::planner/service_resolution`; `anolisa-platform::systemd` |
| `env --verbose` | 环境事实探测 | `anolisa-env`; `anolisa-cli::commands::tier1::env` |
| `info` | CLI/catalog/state/env 摘要 | `anolisa-cli`; `anolisa-core::catalog/state`; `anolisa-env` |
| `update self/runtime/all` | 统一更新 surface | `anolisa-core::update_planner/distribution/state`; `anolisa-platform`; `anolisa-cli` command reshape |

### 9.3 Subscription

| 命令/参数 | Spec 语义 | 开发归属 |
|---|---|---|
| `subscription register --org --key --server` | 注册机器/用户，写 credentials 和 subscription state | `anolisa-cli::commands::subscription`; `anolisa-core::subscription`; `anolisa-platform` credential path/permission |
| `subscription unregister --force` | 注销订阅绑定 | `anolisa-core::subscription/state` |
| `subscription status` | 查询 entitlement 和统计管理状态 | `anolisa-core::subscription`; `anolisa-cli` formatter |
| `subscription refresh` | 刷新 entitlement/offline token | `anolisa-core::subscription`; network client 后续模块 |

第一阶段未实现时统一返回 `NOT_IMPLEMENTED`。

### 9.4 Adapter / TargetProvider

| 命令/参数 | Spec 语义 | 开发归属 |
|---|---|---|
| `adapter scan` | 探测 target provider | `anolisa-env::probes::frameworks`; `anolisa-core::target_provider` |
| `adapter list` | 列出 available/installed bindings | `anolisa-core::catalog/state/target_provider`; `anolisa-cli` formatter |
| `adapter install <COMPONENT> <FRAMEWORK>` | 建立 binding，可能修改目标平台配置 | `anolisa-core::target_provider/planner/backup/transaction`; `anolisa-platform` fs/permissions |
| `adapter remove <COMPONENT> <FRAMEWORK>` | 移除 binding，恢复 ANOLISA backup | `anolisa-core::target_provider/rollback/state`; `anolisa-platform` |

### 9.5 Self

| 命令/参数 | Spec 语义 | 开发归属 |
|---|---|---|
| `self update` | 兼容 alias，长期收敛到 `update self` | `anolisa-cli`; `anolisa-core::self_update/distribution` |
| `self adopt --scan --confirm` | 纳入 existing components 到 installed state 和统计管理范围 | `anolisa-core::adopt/state/subscription`; `anolisa-env`; `anolisa-cli` |
| `self completions <SHELL>` | shell completion | `anolisa-cli` |

### 9.6 Runtime

| 命令/参数 | Spec 语义 | 开发归属 |
|---|---|---|
| `runtime install <COMPONENT|all>` | 直接安装 component，默认 prebuilt | `anolisa-core::runtime_planner/distribution/transaction`; `anolisa-platform` |
| `runtime install --from-source` | 源码构建安装 | `anolisa-build`; `anolisa-core::runtime_planner` |
| `runtime install --from-rpm` | 指定系统包来源 | `anolisa-platform::package_manager`; `anolisa-core::distribution` |
| `runtime install --component-version <VERSION>` | 安装指定组件版本 | `anolisa-core::distribution/version_resolution` |
| `runtime remove <COMPONENT> --purge` | 删除 component，purge 删除 owned config/state | `anolisa-core::transaction/rollback/state` |
| `runtime update <COMPONENT|all>` | 兼容 alias，长期收敛到 `update runtime` | `anolisa-core::update_planner` |
| `runtime build <COMPONENT|all> --release --debug --no-install` | develop/source-build | `anolisa-build`; `anolisa-core::build_plan` |
| `runtime list --available` | component catalog | `anolisa-core::catalog`; `anolisa-cli` formatter |
| `runtime status [COMPONENT]` | component state + health check | `anolisa-core::state/health`; `anolisa-platform` |

### 9.7 Osbase

| 命令/参数 | Spec 语义 | 开发归属 |
|---|---|---|
| `osbase kernel install/remove/status` | ANOLISA kernel/modules/eBPF base | `anolisa-core::osbase_planner`; `anolisa-platform`; `anolisa-env` |
| `osbase sandbox install/remove/list/status <TARGET>` | sandbox substrate | `anolisa-core::osbase_planner`; `anolisa-env::gate`; `anolisa-platform` |
| `osbase sandbox list --available` | 可用 sandbox backend | `anolisa-core::catalog`; `anolisa-env` |
| `osbase security install/remove/status <TARGET>` | loongshield/seccomp 等 security overlay | `anolisa-core::osbase_planner`; `anolisa-platform` |

未实现时统一返回 `NOT_IMPLEMENTED`，但 `list/status` 可以先做只读。

## 10. 第一批开发任务

按 spec 拆分后的最小开发顺序：

1. `CommandSpec` 表落地，当前 `--help` 全部命令都有 public spec。
2. `CliContext` 落地，global flags 传入所有 handler。
3. `NotImplemented` 统一错误落地，未实现命令不能 panic。
4. `OutputModel` 落地，human-first formatter + optional JSON formatter。
5. `Catalog` 分开加载 capability/component/target provider/distribution manifest。
6. `EnvService.detect` 跑真实 probe。
7. `CentralLogStore`、`InstalledState`、`BackupStore` 基础 schema 落地。
8. `CapabilityService.list` 输出 capability availability。
9. `CapabilityService.plan_enable` 支持 `agent-observability --dry-run`。
10. `DistributionResolver` 默认选择预编译产物。
11. `runtime install --version` 改为 `--component-version`，修复 help panic。
12. CLI snapshot tests 覆盖 help、人类输出、JSON 输出、not implemented 输出。

## 11. 已确认的补充决策

本轮评论已收敛为以下开发契约：

1. `logs` 是中心化日志入口，包含 ANOLISA operation/audit logs 和组件主动上报日志；`logs xxx` 表示过滤，help 应从 `<CAPABILITY>` 调整为 `[OBJECT]`。
2. `update all` 不包含 `self`；`anolisa update self` 是独立自更新入口。
3. `doctor` 无参数是只读检查；`doctor --fix` 直接修复；`doctor --dry-run --fix` 是无效组合。

## 12. `src/anolisa` 内落地路径

具体 spec 文档和 TOML 模板已按开发 owner 分配到 `src/anolisa`：

| 路径 | 内容 | 对应 spec |
|---|---|---|
| `src/anolisa/README.md` | 本地开发入口和阅读顺序 | 全局 |
| `src/anolisa/crates/anolisa-cli/SPEC.md` | 命令面、输出、`NOT_IMPLEMENTED` | S1, S2 |
| `src/anolisa/crates/anolisa-core/SPEC.md` | catalog/resolver/planner/state/central log | S3, S5, S6, S8 |
| `src/anolisa/crates/anolisa-env/SPEC.md` | EnvFacts、probe、gate | S4 |
| `src/anolisa/crates/anolisa-platform/SPEC.md` | FsLayout、package manager、distribution install | S7, S8 |
| `src/anolisa/crates/anolisa-build/SPEC.md` | `--from-source`、runtime build、legacy build backend | S7 |
| `src/anolisa/manifests/SPEC.md` | Capability/Component/DistributionIndex/TargetProvider manifest 口径 | S3, S7, S9, S10 |
| `src/anolisa/templates/*.toml` | command、manifest、DistributionIndex、installed state 模板 | S1, S3, S7, S8, S9 |

注意：模板 TOML 不放在 `manifests/runtime`、`manifests/osbase`、`manifests/capabilities` 下，避免当前 loader 将模板当作真实 manifest 读取。
