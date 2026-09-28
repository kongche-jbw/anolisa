# AW 用户指南

[English](../../en/user-entrypoint/aw.md)

AW 用一份配置管理不同 Agent 的工具 Hook。这个实验分支为 QwenPaw、Qoder CLI、
OpenClaw、Hermes 提供独立服务和启动器。用户继续使用 Agent 的原生界面，AW 在原生
工具前后点位运行配置中的命令，并记录调用元数据。

统一配置和命令传输已经可以运行。命令若要返回控制决策，仍需遵循所选 Agent 的原生
响应格式。跨框架的安全策略接口、sec-core 联合交付及一致的动作语义仍待完成。
当前是 Linux 开发版本，尚无安装器或服务发行包。

## 当前分支的可用范围

✅ 表示已在所列范围演示；❌ 表示未实现或未验证。首批交付仍包含全部四个框架。

| 能力 | 状态 |
| --- | --- |
| 一份 `aw.yaml`、命名 Provider、四个 Agent 目标 | ✅ |
| 独立 daemon、命令执行、元数据审计 | ✅ 实验能力 |
| 原生工具前后绑定与既有 Hook 共存 | ✅ 适配器和原生测试；真实模型范围见下表 |
| 声明全部 16 个事件名 | ✅ 静态校验；运行时目前只绑定 `tool.before`、`tool.after` |
| 所有框架共用一种 Provider 响应 | ❌ 结构化 Provider 执行留待下一阶段 |
| AW 管理审批、末尾安全检查、OS 防护 | ❌ |
| 接入既有 Gateway、发行包安装、策略热更新 | ❌ |

| 受测框架 | 原生调度 | 真实模型证据 | 审批边界 |
| --- | --- | --- | --- |
| Qoder CLI 1.1.64 | 默认并行；匹配组的 `sequential: true` 使匹配的同步 Hook 串行 | before 并行重叠、串行参数修改和 headless ask 已观察；❌ 原生权限拒绝了工具，真实 after 采用待补 | 原生 ask 进入权限流程；headless 拒绝，交互批准未验证 |
| OpenClaw 2026.9.6、Node 24.16.0 | before 按优先级串行；after 并发；结果 middleware 按注册顺序串行 | ✅ AW 启动隔离 Gateway，参数修改及替换结果被模型采用 | 原生 `requireApproval`；无模型测试覆盖 deny/report，交互批准未验证 |
| QwenPaw 2.2.2b4、AgentScope 2.0.8 | middleware 嵌套：before A/B，after B/A | ✅ 经 AW 启动公开 `QwenPawAgent` 运行时，验证允许与拒绝；完整 CLI/TUI 未认证 | ❌ 此 middleware 点位没有命令 ask 桥；显式 ask 明确报错 |
| Hermes 源码 `952c941e741e922a9be8fc403c8944c6e96318bb` | shell 回调按注册顺序；工具自身调度不变 | ✅ 经 AW 启动真实 CLI，验证允许、阻断及非交互审批拒绝 | 原生 `approve` 在无交互审批桥时拒绝；交互批准未验证 |

这些结果不代表所有工具类型、失败响应和交互模式都已通过。QwenPaw 与 Qwen Code 是不同框架。

## 构建并准备配置

先单独安装和配置所选 Agent，包括模型访问。这个 fork 分支使用固定 Rust 工具链从源码
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
另一份 `aw-provider/v1alpha1` 示例描述后续结构化执行，当前原生启动器不能执行它。

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

OpenClaw 示例启动新的 Gateway，通过其原生客户端交互。基础配置中的工作目录及文件
引用应使用绝对路径。已有插件 allowlist 会保留。受测版本的 `agent exec` 会遗漏 Hook
插件，因此拒绝此入口。停止本次启动的 Gateway 后，AW run 随之结束。

Hermes 保留原生 shell Hook 授权流程。确认生成的命令设置后再授权；AW 不会自动加上
`--accept-hooks`。

QwenPaw 使用独立、已初始化的工作目录。启动时安装原生插件，Agent 正常退出后删除：

```bash
QWENPAW_WORKING_DIR=/absolute/path/isolated-qwenpaw ./target/debug/aw run qwenpaw --config ./target/aw.yaml
```

如果已有 `plugins/aw-native`，启动器会拒绝覆盖。真实模型验收使用公开运行时 harness，
原生 CLI 启动仍需单独验收。此适配器不支持 `--native-config`。

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

当前只绑定通配工具匹配。启用未支持事件、结构化步骤或 guard 都会拒绝接入，不会暗中
启用 `security.violation` 末尾检查或 sec-core 规则。交互 ask 和跨框架安全响应需要
独立实现与验收。

字段限制及 16 事件词汇见[配置参考](../../../developer-guide/zh/aw/configuration.md)。
下一阶段应补齐 Qoder 真实 after 缺口，并以已核实的原生差异为约束，确定结构化
Provider 请求和响应。
