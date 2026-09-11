# AW 工具结果检查

[English](../../en/user-entrypoint/aw.md)

AW 让显式配置的 Qoder 或 Codex hook 通过 SecCore 检查工具输出，并保留可核验的执行记录。
原始工具结果保持不变。检查用于观察：提示不代表执行批准，也不证明 Agent 展示或采纳了提示。

这是实验性的 Linux 开发者入口。AW 尚未注册 `anolisa install` 或发行 RPM。
在仓库根目录构建已有源码：

```bash
cargo build --manifest-path src/aw/Cargo.toml --locked -p aw-hook-cli
src/aw/target/debug/aw-hook-cli --help
```

## 调用方式

```text
aw-hook-cli <qoder|codex> /absolute/SETTINGS.json
```

Agent 通过 stdin 传入一个原生 `PostToolUse` JSON 对象，然后关闭管道。
输入和配置各有 1 MiB 编码上限；stdin 必须在五秒内结束。
重复 JSON 键（包括嵌套键）会被拒绝；原生 Unicode 键及有限数值元数据会保留。

只接受[原生 adapter profile](../../../../src/aw/docs/design/native-adapters_zh.md)
支持的文本字段。两个宿主均要求原生 `session_id` 和 `tool_use_id`。
Codex 还要求原生 `turn_id`。Qoder 要求 launcher 提供 `qoder_single_turn_id`，
并负责在一轮结束后终止该次调用；不支持跨交互轮次复用同一配置。

CLI 不安装 hooks、不创建 Agent 会话，也不替代现有 SecCore 安全插件。
接入 launcher 必须使用宿主支持的 hook 配置注册命令、绑定存活的 Agent 身份并保护配置。
手工构造 payload 的测试不证明 Agent hook 已安装或其响应已展示。

## 配置参考

配置必须是绝对路径的普通文件，不能是符号链接；所有者必须与 hook 用户相同，
且不能有 group/other 权限（通常为 `0600`）。未知字段会被拒绝。
父目录和 Journal 必须放在 Agent 无写权限的位置；仅文件权限不能隔离同用户 Agent。

| 字段 | 必需语义 |
| --- | --- |
| `runtime` | launcher 认证的 `runtime-binding/v1` 快照；`observation_source` 为 `owned_child`，`process_ref` 为 `pid:<agent_pid>@<agent_start_ticks>` |
| `scope` | 与 runtime 匹配的可信 AW scope；原生 session/tool/turn ID 可填充未设置字段，但不能覆盖冲突值 |
| `agent_pid` | 存活 Agent PID，必须是 hook 的祖先进程 |
| `agent_start_ticks` | Linux `/proc/<pid>/stat` 第 22 字段，用于核对当前进程代次 |
| `qoder_single_turn_id` | Qoder 必需的非空 launcher 单轮标识；Codex 可省略 |
| `journal` | 私有 Core `FileJournal` 的绝对目录 |
| `provider` | 下表规定的显式原生启动配置 |
| `include_low_confidence` | 布尔值；显式选择后才启用原生低置信度选项 |

`runtime` 和 `scope` 必须满足[已注册合同](../../../../src/aw/schemas/)。
[合同 fixture](../../../../src/aw/tests/fixtures/contracts.json) 可用于理解结构，
其中的合成身份不能复制为真实会话。库的 `process_identity(pid)` 为可信 launcher
提供 Linux 父 PID 和启动 ticks。祖先检查用于发现错绑，不会认证恶意进程提供的任意 runtime 声明。

| `provider` 字段 | 必需语义 |
| --- | --- |
| `provider_id`, `provider_version` | 操作者选择的 registry 身份和 Provider release |
| `program`, `program_sha256` | 绝对可执行路径及独立提供的预期小写 SHA-256 |
| `cwd` | 绝对原生工作目录；保留原生配置解析方式 |
| `args` | `--version` 或 `scan-pii` 操作之前的显式前缀参数 |
| `environment` | 完整环境键值；不隐式继承任何环境 |
| `pins` | 最多 32 个指定文件，各含绝对 `path` 及 `state`：`{"sha256":"<expected digest>"}` 或 `"absent"` |
| `limits.timeout_ms` | 原生调用预算，1–300000 ms；版本探测使用独立的调用预算 |
| `limits.input_bytes` | 精确原生 stdin 上限，1–4194304 bytes |
| `limits.output_bytes` | 原生 stdout 上限及本 hook 的 AW 输出预算，1–4194304 bytes |
| `limits.stderr_bytes` | 丢弃的原生 stderr 上限，1–4194304 bytes |

实际版本探测必须返回 `agent-sec-cli 0.12.0`。提供该原生安装所需的完整环境，
包括标准用户规则依赖的真实 `HOME`。AW 不选择另一个用户 home，也不静默忽略损坏的规则配置。
固定可执行程序，以及所声明安装相关的依赖和规则文件；显式 `"absent"` 可发现新增文件。
每个固定文件必须是至多 64 MiB 的普通文件。每次调用前后均复核指定文件。
这不等于完整解释器依赖认证，也不能防止执行期间的变更。主动更新版本或配置后需要重新准入。

## 结果、失败与退出

| 结果 | stdout / 退出码 |
| --- | --- |
| 已核验 clean 检查 | `{}` / `0` |
| 已核验 suspicious 或 sensitive 检查 | 通用 `systemMessage` / `0` |
| Provider、身份、输入、记录或检查失败 | 通用 unavailable `systemMessage`，stderr 有界诊断 / `1` |

响应不含原始工具文本、原生证据片段或 Provider stderr，
不返回替换 payload、allow/deny 决策或采用声明。
原生宿主决定如何展示消息或处理 hook 非零退出。

Core 在分发扫描前持久记录固定计划和 invocation，之后保存 receipt 与终态事实。
Journal 包含摘要和 receipt，不存原始检查输入或输出。失败或中断的事件仍保持占用，
不能自动重放。Core 占用事件之前的失败（包括版本探测失败）没有执行 receipt。

CLI 收到 SIGTERM/SIGINT 后请求取消。Host 停止后续启动、中断管道交换，
回收自己创建的进程组，并允许至多一秒额外清理核验。
外部 hook timeout 必须覆盖输入读取、版本探测、检查、清理和本地记录开销。
SIGKILL、离开受控进程组的后代及内核阻塞操作不在保证范围内。
这套进程处理不是 OS 安全沙箱。

禁用时，只移除所属 launcher 中的 AW hook 注册，保留已有原生安全插件。
需要防重复执行和检查历史时继续保留 Journal；删除 Journal 也会移除旧事件的这些保证。

取消、进程所有权和验证详情见 [Host 架构与接入](../../../../src/aw/docs/design/sec-core-host_zh.md)。

## 显式集成 smoke

常规 AW 门禁运行真实本地协议测试进程和 smoke 校验器自测，不要求 Agent 登录或原生 SecCore 依赖。
如需执行本仓库的实际扫描器，先准备其 frozen Python 3.11.6 安装，再显式传入解释器：

```bash
python3 src/aw/tests/native_smoke.py --host codex --mode payload --provider-python /absolute/venv/bin/python
python3 src/aw/tests/native_smoke.py --host codex --mode agent --provider-python /absolute/venv/bin/python
```

`payload` 构造 hook 输入，验证真实 Host/扫描器/Journal 后半链。
`agent` 运行所选真实 CLI；Codex 使用两次本地脚本化模型响应和 ephemeral 会话，不属于真实模型评测。
Qoder 使用其原生模型和隔离配置目录；缺少登录或 hook 未触发均返回非零。两种模式均不改用户 hook 配置。

Codex 默认 `--codex-sandbox read-only`。宿主无法启动沙箱时，可显式选择
`--codex-sandbox danger-full-access` 运行无沙箱 fixture；不会自动 fallback。
唯一脚本化工具命令打印合成凭据。该运行证明观察链和原始结果保留，不证明沙箱防护或执行批准。
脚本输出进程/端口归属与结果，核对原生审计、精确 hook 文本和 Journal，并删除临时目录。
它保留真实 `HOME`，但将用户规则查找指向临时的缺失文件，将审计和 telemetry 指向 fixture 目录。
这验证真实扫描链路，不覆盖操作者当前的用户规则集。
