# AW 用户指南

[English](../../en/user-entrypoint/aw.md)

AW 用一份配置管理不同 Agent 的工具 Hook。这个实验分支为 QwenPaw、Qoder CLI、
OpenClaw、Hermes 提供独立服务和启动器。用户继续使用 Agent 的原生界面，AW 在原生
工具前后点位运行配置中的命令，并记录调用元数据。

同一个结构化 Provider 已可在四个适配器中完成工具前观察/阻断、工具后观察；
既有原生脚本继续支持。完整 sec-core 联合交付及一致的动作语义仍待完成。
当前是 Linux 开发版本，尚无安装器或服务发行包。

## 当前分支的可用范围

✅ 表示已在所列范围演示；❌ 表示未实现或未验证。首批交付仍包含全部四个框架。

| 能力 | 状态 |
| --- | --- |
| 一份 `aw.yaml`、命名 Provider、四个 Agent 目标 | ✅ |
| 独立 daemon、命令执行、元数据审计 | ✅ 实验能力 |
| 原生工具前后绑定与既有 Hook 共存 | ✅ 适配器和原生测试；真实模型范围见下表 |
| 声明全部 16 个事件名 | ✅ 静态校验；运行时目前只绑定 `tool.before`、`tool.after` |
| 所有框架共用一种 Provider 响应 | ✅ 工具前观察/阻断、工具后观察；保留原生调度 |
| 复用持久 Agent profile | ✅ OpenClaw 的 `--native-state-dir`、QwenPaw 工作目录、Qoder 原生 profile；❌ Hermes 既有 profile 接入 |
| 启动时确认插件加载 | ✅ OpenClaw Gateway 启动回执、QwenPaw middleware 注册回执；单次效果仍需工具回执证明 |
| AW 管理审批、末尾安全检查、OS 防护 | ❌ |
| 接入既有 Gateway、发行包安装、策略热更新 | ❌ |

| 受测框架 | 原生调度 | 执行证据 | 审批边界 |
| --- | --- | --- | --- |
| Qoder CLI 1.1.64 | 默认并行；匹配组的 `sequential: true` 使匹配的同步 Hook 串行 | ✅ before 并行重叠和串行参数修改；print/TUI 模型采用原生模式 after 替换结果；headless ask 拒绝 | 原生 ask 进入权限流程；headless 拒绝，交互批准未验证 |
| OpenClaw 2026.9.6、Node 24.16.0 | before 按优先级串行；after 并发；结果 middleware 按注册顺序串行 | ✅ AW 启动 Gateway，参数修改及替换结果被模型采用；连续启动保留原生认证、会话和既有插件 | 原生 `requireApproval`；无模型测试覆盖 deny/report，交互批准未验证 |
| QwenPaw 2.2.2b4、AgentScope 2.0.8 | middleware 嵌套：before A/B，after B/A | ✅ 官方 App/API 加真模型验证公共策略允许/阻断/after、既有插件共存及工作目录文件保留；❌ ACP/TUI 插件未加载 | ❌ 此 middleware 点位没有命令 ask 桥；显式 ask 明确报错 |
| Hermes 源码 `952c941e741e922a9be8fc403c8944c6e96318bb` | shell 回调按注册顺序；工具自身调度不变 | ✅ 经 AW 启动真实 CLI，验证允许、阻断及非交互审批拒绝 | 原生 `approve` 在无交互审批桥时拒绝；交互批准未验证 |

通用 Provider 已在 Qoder、官方 QwenPaw App、Hermes CLI 和 OpenClaw Gateway 中
通过真模型允许/阻断/after 验证，并与既有 Hook 共存。OpenClaw、Hermes、QwenPaw
都可能对被阻断的尝试发送 after，出现 after 不等于工具已经执行。
另有真实 Qoder 采用本机 sec-core V1 0.8.0 判断的联合证据。
这些结果不代表所有工具类型、失败响应和交互模式都已通过。QwenPaw 与 Qwen Code 是不同框架。

## 构建并准备配置

先单独安装和配置所选 Agent，包括模型访问。框架支持时可以复用同一模型服务凭据；
原生模型与工作目录设置独立于公共 AW 策略。这个 fork 分支使用固定 Rust 工具链从源码
构建。在仓库根目录执行：

```bash
cd src/aw
cargo build --locked -p aw-cli
cp crates/aw-cli/examples/aw.native.yaml ./target/aw.yaml
./target/debug/aw validate --config ./target/aw.yaml
./target/debug/aw plan qoder --config ./target/aw.yaml
```

[原生示例](https://github.com/kongche-jbw/anolisa/blob/feat/aw/native-hook-lab/src/aw/crates/aw-cli/examples/aw.native.yaml)
包含四个 Agent 和两个无副作用的命令 Provider。可执行文件不在 `PATH` 时，修改对应
`agents.<id>.argv`。`validate` 只检查语法与引用，不运行命令；`plan` 另检查适配器的
原生注册限制。两者都不能证明模型已经采用某种效果。

`apiVersion`、`kind`、`metadata`、`spec` 借鉴 Kubernetes 的命名；AW 不需要集群或
CRD。`spec.providers` 是命名对象，事件步骤通过名字引用。例如：

```yaml
providers:
  my-hook:
    protocol: native-hook/v1alpha1
    transport:
      type: stdio
      location: agent
      argv: [/usr/local/bin/my-hook, --mode, inspect]
      env:
        POLICY_MODE: observe
    timeout_ms: 5000
    max_output_bytes: 1048576
    config: {}
events:
  tool.before:
    enabled: true
    required: true
    steps:
      - id: inspect-tool
        provider: my-hook
        native: {}
```

这段配置放在 `spec` 内。命令从 stdin 接收适配器事件，AW 将 stdout、stderr 和退出码
返回适配器。参数按字面传递；需要 shell 语法时显式选择 `/bin/sh -c`。环境值同样按
字面传递，不展开 `$NAME`。命令继承 Hook 回调进程的环境和工作目录，再应用配置中的
`env` 覆盖值。

仅观察的命令可以返回空 stdout 和退出码零。阻断或修改响应必须符合框架的原生合同。
原生步骤没有 `operation`、`effects` 或私有 `config`，Provider 的 `config` 必须为 `{}`。
跨框架观察/阻断使用下节的结构化示例。较大的静态 Schema 示例仍含本运行时拒绝的
guard 和变换效果。

## 在不同 Agent 中复用策略

复制 [aw.provider.yaml](https://github.com/kongche-jbw/anolisa/blob/feat/aw/native-hook-lab/src/aw/crates/aw-cli/examples/aw.provider.yaml)，
将 argv 中的 `examples/providers/policy.py` 改为实际绝对路径。Python 3 是这个可替换
示例 Provider 的依赖，Rust 启动器不依赖它。示例阻断参数含 `AW_DENY_FIXTURE` 的工具，
并观察已完成工具；可用无害打印命令试验。

```bash
cp crates/aw-cli/examples/aw.provider.yaml ./target/aw.policy.yaml
# Edit the absolute Provider path in ./target/aw.policy.yaml.
./target/debug/aw check qoder --config ./target/aw.policy.yaml
./target/debug/aw run qoder --config ./target/aw.policy.yaml
./target/debug/aw run openclaw --config ./target/aw.policy.yaml --native-config /absolute/path/openclaw.json
```

同一文件也包含 Hermes 和 QwenPaw，按下文准备原生配置即可。`check` 执行 Provider
发现和私有配置校验，不启动 Agent；`run` 自动执行相同检查。每次回调提供公共事件，
只接受配置允许的效果。空 effects 保留宿主权限检查。统一协议保留原生调度，不增加
交互 ask、变换或末尾 guard。

Provider 失败按 `on_error` 处理：工具前阻断，或显式 `report`。仍存活的 `aw hook`
也会映射 daemon 失败。Qoder helper 缺失或被杀时，宿主仍可能放行，配置已安装不代表
强制安全防护。审计区分策略阻断与 Provider 故障，实际采用仍需框架证据。

自定义 Provider 遵循[协议](https://github.com/kongche-jbw/anolisa/blob/feat/aw/native-hook-lab/src/aw/docs/design/provider-protocol_zh.md)。
可选 sec-core CLI 示例使用独立规则配置，不表示完整安全策略已交付。较大的 Schema
示例仍用于说明规划能力，不能作为本切片的可运行策略。

## 启动 Agent

在 `src/aw` 下，准备好复制的文件及已安装、认证的 Qoder CLI：

```bash
./target/debug/aw run qoder --config ./target/aw.yaml
```

AW 检查注册项，生成私有原生 Hook 设置，启动或复用相同配置版本的 daemon，再启动
Qoder 并继承终端输入输出。Agent 退出后，AW 将其退出码返回 shell，并删除生成的设置。
Ctrl-C 和终止信号会转发到所属 Agent 进程组；shell 任务挂起/恢复（Ctrl-Z）尚未实现。
既有 Hook 仍按 Qoder 自身规则调度。Qoder 1.1.64 的 `--setting-sources` 会排除生成的
`--settings` Hook，因此该组合会被拒绝，包括 `--` 后传入的参数。`--settings`
也由 AW 管理；原有 Qoder 设置通过 `--native-config` 提供，不重复传入此原生参数。

OpenClaw 和 Hermes 需要显式原生基础配置。这些文件保存 Agent 的模型和运行设置，
不替代公共 AW 策略。AW 将其复制到私有启动配置，再添加自己的注册项：

```bash
./target/debug/aw run openclaw --config ./target/aw.yaml --native-config /absolute/path/openclaw.json
./target/debug/aw run hermes --config ./target/aw.yaml --native-config /absolute/path/hermes.yaml
```

OpenClaw 示例启动新的 Gateway，通过其原生客户端交互，默认使用临时原生状态。
需要跨次启动保留认证、会话和已安装插件时，指定原有 OpenClaw 状态目录：

```bash
./target/debug/aw run openclaw --config ./target/aw.yaml --native-config /absolute/path/openclaw.json --native-state-dir /absolute/path/openclaw-state
```

使用该目录前先停止原来的 Gateway。AW 启动新进程，不接管已运行的 Gateway。
原配置和选定状态目录保留，退出时只删除生成配置和 AW 桥。profile 下的
`.aw-launch.lock` 防止多个 AW 同时使用该目录，退出后保留未加锁文件。此模式保留原生
home 和 cwd 及其路径解析语义。请提供不含 `$include` 的展开 JSON；当前不重定位原生
include 引用。

AW 最多等待 30 秒，确认 OpenClaw 插件发出 `gateway_start` 回执。回执缺失或无效时，
启动失败并回收本次 Agent 进程组。全局关闭插件或 deny AW 会被拒绝。原 allowlist 和
插件条目保留，既有 Hook 代码不会自动转换为 Provider。`agent exec`、`--profile`、
`--dev`、`--reset`、`--container`、`--force` 会遗漏受支持的接线或与进程管理冲突，
因此拒绝；容器或服务管理环境覆盖接线的情况同样拒绝。停止本次 Gateway 后 AW run 结束。

Hermes 保留原生 shell Hook 授权流程。AW 拒绝会关闭 Hook 注册的 `HERMES_SAFE_MODE` (`1/true/yes/on`)。确认生成的命令设置后再授权；AW 不会自动加上
`--accept-hooks`。
该 Hermes 版本从 `HERMES_HOME` 同时读取配置和持久状态，没有可用的
`HERMES_CONFIG_PATH` 覆盖接口。AW 仍使用隔离 Hermes home，不复用原有认证和会话；
Hermes 的 `--native-state-dir` 会明确拒绝。已有 profile 接入需要安装原生插件，
或由上游增加配置覆盖接口。

QwenPaw 使用 `qwenpaw app` 服务入口及独立、已初始化的工作目录。受测版本的裸
`qwenpaw`、项目目录、TUI 和 ACP 入口会遗漏外部 Hook 插件，AW 拒绝这些入口。
启动时安装原生插件，Agent 正常退出后删除：

```bash
QWENPAW_WORKING_DIR=/absolute/path/isolated-qwenpaw ./target/debug/aw run qwenpaw --config ./target/aw.yaml
```

如果已有 `plugins/aw-native`，启动器会拒绝覆盖；选定工作目录内其他插件和持久文件
保留。此适配器不支持 `--native-config`、`--native-state-dir`。AW 最多等待 30 秒，
确认全部 middleware 工厂完成注册；该检查不控制 App HTTP 端口开放时机，也不证明后续
每次调用采用策略。发送请求前，确认原生插件状态中 `aw-native` 已 loaded；真模型验收
同时核对该状态和实际工具回执。按原生 middleware 嵌套语义，被拒绝的工具仍可能产生
可观察的 after 响应。

## 服务生命周期与记录

`--state-dir`、`--socket` 分别覆盖 `spec.daemon.state_dir`、`endpoint`。使用 `auto`
时，AW 在 `XDG_RUNTIME_DIR` 下按配置版本建立目录，socket 为其中的 `aw.sock`。
显式 endpoint 是绝对 Unix socket 路径，可带 `unix://` 前缀。目录仅当前用户可访问。

需要显式管理服务时，准备私有绝对路径，在两个终端中执行：

```bash
./target/debug/aw serve --config ./target/aw.yaml --socket /absolute/private/aw.sock --idle-timeout 300
./target/debug/aw status --socket /absolute/private/aw.sock
./target/debug/aw stop --socket /absolute/private/aw.sock
```

设置 `daemon.startup: external` 后，`run` 必须找到已有服务；否则按需启动。活跃启动器
租约和回调会阻止空闲退出。最后一个会话结束后，按需服务最多再保留 300 秒。
`status` 报告服务身份、活跃回调及租约，不代表策略效果或 Agent 健康认证。

socket 同目录的 `audit.jsonl` 记录配置版本、Agent、事件、Provider、时长、字节数和
退出结果，不记录事件正文、参数和环境。Agent 和 Provider 日志独立管理，可能包含它们
自己的数据。该审计是本地元数据记录，并非不可篡改的安全日志。删除状态目录前先停止
自己启动的服务。强制终止启动器可能留下生成配置，应在所属 Agent 停止后清理。

## 原生限制与下一阶段

每条命令受 Provider 超时和输出大小限制，宿主自身的回调预算也生效。原生 Hook 保留
原来的顺序、错误处理与决策聚合，AW 不施加整事件的共享期限。必填字段
`default_event_budget_ms` 在原生模式中保留但不执行；显式事件 `budget_ms` 会被拒绝。
`required` 不会增强宿主的控制保证。

当前只绑定通配工具匹配。启用未支持事件、效果或 guard 都会拒绝接入，不会暗中
启用 `security.violation` 末尾检查或 sec-core 规则。交互 ask、更强执行保证及完整
安全覆盖仍需独立实现与验收。

字段限制及 16 事件词汇见[配置参考](../../../developer-guide/zh/aw/configuration.md)。
后续补齐四框架同配置真实场景和 QwenPaw 完整入口，继续推进 sec-core 联合规则及
发行管理。
