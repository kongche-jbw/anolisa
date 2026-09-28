# 原生 Hook 接入实验

[English](native-hooks.md)

这个 fork 在结构化 Provider 协议完成前，交付一个可运行切片：统一配置、独立 daemon、
Rust 启动器及四个 Agent 框架的工具回调。它不依赖 cosh 或 Herdr。

## 执行边界

`aw run` 读取配置，准备隔离原生配置，获取 daemon 会话租约，再用原生终端启动 Agent。
每个注册的原生回调调用 `aw hook`，客户端将回调正文、工作目录和环境传到私有 Unix
socket。daemon 执行一条配置中的 argv，限制管道大小与执行时间，记录元数据，并返回
stdout、stderr、退出状态。回调顺序和响应解释由原生框架决定。

启动器用 Rust 实现。OpenClaw 的 JavaScript 插件和 QwenPaw 的 Python 插件由宿主加载，
不是外部产品启动脚本。Hermes 和 Qoder 使用已有命令 Hook。`tests/native` 下的 Python
文件仅用于验收，产品启动不依赖这些测试脚本。

| 源码 | 职责 |
| --- | --- |
| `crates/aw-config` | 公共配置 Schema 与静态关系 |
| `crates/aw-cli/src/model.rs` | 原生绑定准入与命令查找 |
| `crates/aw-cli/src/launch.rs` | 生成原生配置、服务就绪与 Agent 进程 |
| `crates/aw-cli/src/server.rs` | 固定配置服务、租约、元数据审计 |
| `crates/aw-cli/src/ipc.rs` | 有界本地请求响应 |
| `crates/aw-cli/src/process.rs` | 复用 POC 有界进程执行；取消和进程组回收 |
| `adapters` | 最小原生桥及固定框架合同 |
| `tests/native` | 框架原生验收及可选真实模型验收 |

原生模式不调度 AW 策略链，也不替代待合入的 Core/Journal PR。它复用 POC 的有界
进程机制，不将 daemon 绑定到 cosh。`native-hook/v1alpha1` 与 `aw-provider/v1alpha1`
明确区分，原生响应和错误策略仍取决于宿主。后续结构化执行可接入 Core、标准事件和
效果准入，不暗中改变当前模式。

## 原生合同

| 框架 | 输入与响应 | 调度及效果边界 |
| --- | --- | --- |
| Qoder 1.1.64 | 原生 PreToolUse/PostToolUse JSON 与命令退出状态 | 默认并行；任一匹配同步组声明 `sequential` 后，该匹配集合串行。串行 before 传递 `updatedInput`；after runner 不将中间改写传给下一个 Hook |
| OpenClaw 2026.9.6 | stdin 为 `{hook,event,context}`；stdout 为原生插件响应 | before 按优先级降序串行，每个处理器看到原始快照；after 并发等待全部结果，忽略返回值 |
| QwenPaw 2.2.2b4 | 原生工具调用和响应的投影；不支持的 ask 明确报错 | 较低 middleware 优先级包裹较高优先级；before A/B、after B/A。原生权限校验在 `on_acting` 之前 |
| Hermes 固定源码 | 原生 shell Hook 事件与 `{action: ...}` 响应 | 回调按注册顺序串行；block 优先于 approve；block 后仍执行后续回调。post 仅观察 |

OpenClaw 另提供两个 `tool.after` 点位。`native.point: tool_result_persist` 安装同步
历史记录投影，不能证明当前模型循环收到修改文本。`native.point: agent_tool_result`
使用原生结果 middleware API，按注册顺序传递结果，改变模型消费的工具结果，没有
priority 字段。每个注册项使用独立 Provider 实例名。

Qoder 的 `native.sequential` 是显式宿主选项；OpenClaw、QwenPaw 使用
`native.priority`。不支持的扩展会使目标准入失败。空 `native: {}` 步骤可在四目标
共用，但结构化 Provider 完成前，控制脚本仍需按框架解析和返回。

## 证据与缺口

验收使用官方 Qoder 1.1.64 Linux ARM64、OpenClaw 2026.9.6 和 Node 24.16.0。
QwenPaw 源码为 `3822ec7173d17cf37c8a02f51d3ed5628079e86e`，配套 AgentScope 2.0.8；
Hermes 源码为 `952c941e741e922a9be8fc403c8944c6e96318bb`。Python 环境独立安装，
没有替换用户的长期安装和服务。

| 验收项 | 结果 |
| --- | --- |
| Qoder before 调度、参数修改、原生 ask | 观察到真实 AW 回调，headless ask 拒绝；原生写入权限阻止工具执行，真实 PostToolUse 采用待补 |
| OpenClaw Gateway 经 `aw run` | 工具参数修改生效；原始文本值 52 经结果 middleware 改为 73，模型回答 73；历史和 after 观察同样记录 73 |
| QwenPaw 公开运行时经 `aw run` | 允许时写入 42；拒绝不写入；显式 ask 报错且不写入。观察到 before A/B、after B/A；完整 CLI/TUI 待补 |
| Hermes 真实 CLI 经 `aw run` | 允许时写入 42；block 和非交互 approve 均不写入；观察到 before A/B、after A/B |
| 原生共存 | 使用安装框架的 runner 和插件加载验证；OpenClaw 缺省或空 allowlist 保留原生插件准入 |
| 用户交互审批 | 四框架均未认证；没有 AW 审批界面 |
| sec-core 策略、全部工具/结果类型、OS/final/protected | 这些命令 fixture 不提供此类认证 |

Qoder 授权上限为三个简短场景，计入更早的启动探测。fixture 已补上显式工具权限，
额度用尽后未再次运行。BYOK 保存失败，原因尚未证明；未经重新授权不再使用 Qoder 额度。

真实模型测试显式运行，不进入 CI。有界 runner 记录所属 PID、命令、期限及清理。
本地证据保留在忽略目录 `target/native-lab/<framework>`，构建和门禁材料位于
`src/aw/target`。凭据由运行时提供，不进入测试 fixture。

常规 AW 门禁无需下载 Agent 或访问模型，覆盖 Rust 配置、传输、进程和启动器。
框架原生测试加载固定的官方运行时；依赖缺失或版本变化必须明确失败，不能替换成模拟器
后报告通过。

## 服务限制

本地 socket 和状态目录仅同一用户可访问。原生回调继承调用方环境，可能包含 Agent
凭据，因此 Provider 处于该用户的信任范围内。daemon 审计不保存正文、环境或 argv。
它不是经认证的效果日志或 OS 安全边界，也不能阻止宿主忽略原生回调。

一个 daemon 使用固定配置，同 socket 不接受另一 revision。活跃会话租约按 PID 与
进程启动时间识别，已退出 owner 会清理。回调和租约存在时服务保持运行；按需模式
空闲 300 秒后退出。控制调用与就绪检查设有期限。当前不包含发行安装、热更新、
既有 Gateway attach 或部署对账。

每条命令的每个输出流最多 4 MiB，外层超时上限 300 秒，再受各框架预算限制。AW 保留
stdout、stderr、原生退出码，适配器和宿主合同决定如何消费。shell 表达式需要显式
启动 shell。清理覆盖所属进程组，主动建立新会话逃逸的进程不在此保证内。

## 后续交付

1. 在显式原生权限和隔离配置下补齐 Qoder 真实 after、QwenPaw 完整入口；交互 ask
   保持独立验收维度。
2. 依据矩阵确定标准事件、`describe/validate_config/invoke`、效果与错误，接入 Core，
   验证宿主真实采用。
3. 与 sec-core 协作完成规则配置、内置/自定义规则聚合、不支持动作的明确处理及真实
   安全案例。AW 末尾检查需要受控执行链，不能从本次宿主调度模式推导。
4. 补充服务发行包、绑定/查询生命周期和四框架同策略验收。cosh、Herdr 后续作为独立
   客户端复用 AW 服务。
