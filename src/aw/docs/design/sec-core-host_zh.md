# SecCore Host 与原生 hook 组合

[English](sec-core-host.md)

Linux Host 通过 AW 执行链调用现有 SecCore PII scanner。显式启用的 hook 将
Codex 或 Qoder 的 `PostToolUse` 文本接入 Host，记录检查并保留原始工具结果。
Provider 声明 `security.content.inspect/v2`、`authority: advise` 和
`guarantee: declared`；它提供动作执行后的内容观测。

## 所有权与依赖

| Crate | AW 依赖 | 职责 |
|---|---|---|
| `aw-contracts` | 无 | 版本化 Schema 与无副作用校验 |
| `aw-core` | Contracts | 固定计划的准备、串行执行及经确认的 Journal 记录 |
| `aw-adapters` | Contracts、Core | 原生文本捕获、身份一致性及检查计划桥接 |
| `aw-sec-core` | Contracts | 内容检查与原生 PII JSON 之间的纯映射 |
| `aw-host-process` | Contracts、Core | 共享启动 pin、有界进程所有权与取消 |
| `aw-sec-host` | Contracts、Core、SecCore 映射、Host process | 原生准入、协议映射及 Provider receipt |
| `aw-tokenless-host` | Contracts、Core、Host process | 原生投影映射及回执；另依赖 Tokenless 纯协议 |
| `aw-hook-cli` | Contracts、Core、Adapters、Host | 显式启用的 hook 配置、实时 owner 校验及单事件组合 |

Core 与 Contracts 不依赖具体 Host。原生协议映射不创建进程，也不实现 scanner。
共享进程 runner 与 hook 仅在 Linux 进程边界使用 `libc`；两者均不引入 async runtime 或另一套
Agent supervisor。统一检查入口约束这些依赖、源码大小及 Host/hook 集成目标非空。

数据路径为：原生 payload → Adapter 捕获 → Core 准备/执行 → Host 进程 → 原生
`scan-pii` → 映射后的 inspection 与 receipt → Core Journal → 原生观测响应。
各层合同见 [Core 执行](core-execution_zh.md)、[原生 adapters](native-adapters_zh.md)
与 [SecCore 映射](sec-core-adapter_zh.md)。

## 原生启动与来源声明

`SecHost::new` 校验声明的配置与 descriptor，检查 executable 及所选文件 pin，
然后用检查调用所使用的同一个有界 runner 执行 `--version`。准入要求退出码为零，
且响应精确为 `agent-sec-cli 0.12.0\n`。Provider release identity 是由操作者另行
选择的值。manifest digest 绑定完整声明的启动配置、原生版本、协议 profile 及能力。

调用由配置的 executable、配置的前缀参数以及下列参数构成：

```text
scan-pii --stdin --format json --source tool_output
```

Host 通过 stdin 发送精确 UTF-8 字节，不添加换行。只有 AW 请求明确选择时，映射层
才添加 `--include-low-confidence`。它不请求原始 evidence 或脱敏替换文本，不关闭
原生 middleware，也不替换为固定 detector 列表。原生用户规则与 middleware 在显式
提供的环境变量和工作目录下运行。

Executable 与工作目录必须为绝对路径。`env_clear` 防止继承未声明的环境变量。
操作者提供预期 executable SHA-256，以及最多 32 项所选文件 pin；每个文件最多
64 MiB。pin 表示精确 SHA-256 或明确的不存在断言。原生交换成功前后进行的检查会
拒绝 executable 字节改变、所选文件改变或不存在断言失效。对于受 Python 模块或
用户规则文件影响的部署，可以将这些文件纳入 pin。

这是所选文件的一致性检查，不证明 interpreter 的完整 import graph。它不能防止
同用户在两次检查之间修改文件、修改后恢复，或访问未 pin 的文件。Settings 与
Journal 必须由 launcher 保持所有权；配置权限和 PID 祖先关系均不构成抵御同用户
攻击者的隔离边界。

## 预算与进程生命周期

Host 在扫描前校验 invocation 绑定与内容摘要。启动 timeout 取配置上限、invocation
wall-time 预算和绝对 deadline 剩余时间的最小值。准备与 pin 检查消耗同一预算，
spawn 前会重新计算剩余时间。produced 结果还必须满足实际观测的 deadline、wall-time
上限及编码后 output 预算。

原生 stdin、stdout 与丢弃的 stderr 分别有独立字节上限。配置允许 1–300,000 ms，
每个流为 1 字节–4 MiB。单线程非阻塞循环交换管道，在每次有界读取之间检查时间；
持续输出不能阻止 deadline 检查。版本探测使用配置中的上限。hook 另将 settings
和原生输入限制为 1 MiB，并最多等待五秒读取原生 stdin。

`SecHost::new_cancellable` 接受共享、由调用方持有的 cancellation port。版本探测
和每次原生调用都在 spawn 前及管道循环之间检查取消，并沿相同路径清理进程。
`SecHost::new` 使用 `NeverCancel`；需要协调退出的接入应用必须提供自己的 port。
该库不安装全局 signal handler。

每个原生 child 创建独立进程组。所有正常或错误返回都尝试向该组发送 `SIGKILL`，
并在暴露响应前验证清理。`waitid(..., WNOWAIT)` 在进程组检查完成前保留组长 PID，
避免进程组定向信号期间的 PID 复用。Host 要求独占 child 回收；接入代码不能使用
全局 `waitpid(-1)` handler 与其争抢。

清理另有一秒轮询预算，procfs 扫描最多 65,536 项。它检查自有进程组是否仍有活跃
任务，包括 zombie 进程 leader 的其他线程，然后非阻塞回收直属 child。无法验证
或未完成清理时返回 `provider_cleanup_failed`，覆盖原本成功的响应。Drop 只进行
非阻塞的尽力清理。

进程组清理不能约束通过 `setsid` 或其他进程组逃逸的后代，也不承诺回收所有孙进程。
对于不可中断的内核任务、阻塞的文件系统操作或进程 spawn，这些用户态 deadline
不能给出硬实时完成保证。无法验证清理会保持可见失败；Host 不提供 cgroup、subreaper
或 sandbox attestation。

## Receipt、Journal 与原生结果

Host 只有在退出、覆盖率、一致性及 Schema 检查通过后才投影原生 output。receipt
绑定 invocation、Provider identity、Schema、input digest、scope 和 plan reference，
随后与 output 一同校验。一般原生失败产生 failed receipt，不带 output；invocation
绑定错误或无法信任 receipt 时返回 Host error。原生 stderr 和 evidence 片段不会进入
错误消息或 hook 响应。

hook 创建一个 required 内容检查步骤，失败策略为 `reject_plan`。它在 Core 分发前
再次检查实时 Agent owner，并在返回前用终态回执核对已存 Journal 链尾。

| 结果 | Journal 与原生响应 |
|---|---|
| 已验证的 `clean` 检查 | 记录执行；响应 `{}`；退出码 0 |
| 已验证的 `suspicious` 或 `sensitive` 检查 | 记录执行；观测 `systemMessage`；退出码 0 |
| Failed receipt | 记录失败及 preserve 决策；不可用 `systemMessage`；退出码 1 |
| Claim 后发生错误 | 已有预留保持完成或中断状态；不可用响应；退出码 1 |
| Claim 前 settings、身份或 Provider 准入失败 | 不要求存在执行 claim；不可用响应；退出码 1 |

响应不替换工具结果，也不请求运行工具的许可。内容 verdict 不构成 dispatch 批准，
也不能阻止已经发生的动作。应保留原生安全 plugin 与最终执行 guard。Journal 记录
元数据、摘要、receipt 和决策；原始 payload 与映射后的 output 仍可通过 Rust 结果
提供给接入应用，由该应用控制保留策略。

## Hook 身份与支持的生命周期

CLI 接受 `qoder` 或 `codex` 以及 settings 绝对路径。Settings 必须是调用方所有的
私有普通文件，最终路径不得是 symlink。Launcher 提供认证后的 runtime/scope 事实、
实时 Agent PID 及 Linux start ticks。hook 要求该 Agent incarnation 位于自身祖先链中，
并核对 runtime process reference。原生 session/tool ID 必须与任何预配置值一致。
事件键来自带 scope 的原生触发；重复事件在重启后仍保持预留。

Codex 提供实际观测的 turn ID。Qoder 要求 launcher 为明确受限的单轮提供
`qoder_single_turn_id`；不支持将其跨多轮 session 复用。自动多轮监管、hook 安装、
PTY/session 管理、pre-tool 强制执行及结果采用均不属于该入口。其他 Adapter profile
的测试不会扩大可执行 hook 所支持的宿主范围。

CLI 将 `SIGTERM` 与 `SIGINT` 转为 Core 和 Host 共享的取消状态，让原生进程清理
先于检查不可用响应完成。Rust 库由接入 owner 负责注册 signal。`SIGKILL` 无法执行
这条清理路径，因此不保证后代清理。
