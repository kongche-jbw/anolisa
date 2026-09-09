# Qoder 与 Codex 检查接入

[English](qoder-codex-inspection.md)

本次把原生后置 hook 接到共享适配层、Core 和实际 SecCore 扫描器，提供可选的观测入口。现有安全插件及其策略继续独立运行。

## 执行路径

`Qoder/Codex PostToolUse → aw-hook-cli → aw-adapters → aw-core → aw-sec-host → SecCore native protocol 1 → Receipt → FileJournal`

`aw-hook-cli` 从 stdin 读取一次原生事件。启动器通过显式配置提供 runtime binding、scope、Agent PID 与启动时刻、Provider 启动参数和证据目录。Linux 下，入口检查自身是否属于指定 Agent 进程代次的后代，再绑定原生会话、工具和 Codex 轮次身份；不一致时拒绝。这是本机进程关联，不代表 OS 防护，也不能防止拥有相同用户权限的其他进程篡改数据。

Qoder 已公开的 hook 数据没有轮次 ID，因此当前测试入口必须显式提供启动器拥有的**单轮身份**，不能把它用于一般多轮对话。Codex 使用原生 `turn_id`。稳定的事件键使同一工具事件无法通过相同 Journal 重复执行。

入口构造固定的 `security.content.inspect/v2` 计划。必需 Provider 失败时拒绝该计划，并保留原生结果。发现敏感内容时只通过 `systemMessage` 提醒，不请求审批、阻断工具、脱敏文本或声明采用。现有安全插件在单独迁移前仍需保留自身策略。为同一能力同时注册新旧检查路径前，必须处理重复扫描和重复提醒。

## 真实 Provider 与原有合同

`aw-sec-host` 通过显式程序、参数数组和环境调用 SecCore native protocol 1，复用组件扫描器与原生响应格式。AW 不实现检测规则或安全策略。核验的 Provider 源码来自 PoC 分支 `5ebfc0b3`，包版本为 `agent-sec-cli` 0.11.0，位置为 `src/agent-sec-core/agent-sec-cli/src/agent_sec_cli/aw_provider/`。

该源码及其 Python 3.11.6 环境是本次接入的外部依赖；本提交没有引入旧 PoC 的 AW Core 或 v1 标准能力合同，也不假定系统安装的 CLI 已包含此原生入口。

Host 将扫描器报告的字节数、截断状态和发现项映射到已有 v2 输出。覆盖信息缺失或矛盾时失败，部分扫描不会被标成完整且无风险。错误、跳过和生成结果分别记录。Receipt 的时间与输入输出摘要来自实际调用和进程交互。

Descriptor 固定显式配置与可执行文件摘要，但不能认证所有 Python 导入及依赖。Python 参数使用 `-P`，避免 Agent 工作目录中的同名模块覆盖指定 Provider。子进程只接收显式环境。Linux 进程组、有界非阻塞管道和截止时间限制本次交互；Host 不安装 OS sandbox，也不能隔离主动逃离进程组的恶意 Provider。

入口先校验 Journal 确认，再保存每次事件的摘要，其中包含 scope、输入摘要及大小、Receipt 和检查输出。执行日志与摘要不保存原始工具文本；扫描器 stderr 也不会进入用户提示。结算前失败可能留下没有终态摘要的 reservation，此时不能盲目重试。原有 Schema 资源和 Core 实现保持不变。

## 构建与试运行

在 `src/aw` 下执行：

```bash
cargo build -p aw-hook-cli --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo doc --workspace --no-deps --locked
python3 tests/check_canonical.py
python3 tests/native_smoke.py --help
```

测试脚本要求显式提供 `aw-hook-cli`、Provider Python 和源码目录的绝对路径，以及一个尚不存在的输出目录。它创建独立 Agent 配置，不复制认证，并保留运行证据。判断结果前，应查看实际命令与生命周期记录。Codex 的模型是本地**脚本化 Responses 测试服务**；Codex 二进制、shell 工具、AW Core、扫描器和 Journal 都是真实实现。这能验收原生接入，但不代表真实远端模型推理已验证。

Qoder 使用独立配置，并关闭会话持久化。缺少认证时记录为阻塞，直接给插件 stdin 输入不能替代 Qoder 实机验收。这两类测试都不证明 Tokenless 压缩、最终采用、Herdr 指标、前置防护或干净 VM 安装完成。

## 原生接口依据

- [Codex hooks](https://developers.openai.com/codex/hooks)：匹配的 command hook 可能并发运行，注册顺序不能代替 AW 的串行执行计划。
- [Codex 0.153.4 原生工具 hook 测试](https://github.com/openai/codex/blob/rust-v0.153.4/codex-rs/core/tests/suite/hooks.rs#L5066)：shell 执行映射为 `Bash`，后置输出是字符串，并提供 `turn_id`。
- [Qoder hooks](https://docs.qoder.com/cli/hooks)：结果形状随工具变化，后置替换使用 `hookSpecificOutput.updatedToolOutput`。

Qoder 的结果替换仍需单独实现交付链。Codex 没有与之相同的通用替换合同。不能仅凭 hook 被执行，就给框架声明 final guard 或采用能力。

## 核验环境与复现命令

本次核验环境为 Linux ARM64，Qoder CLI 1.1.5、Codex CLI 0.153.4。
Codex 通过敏感文本（44 字节）、普通文本（34 字节）和子进程故障注入三种场景。
故障场景在扫描器启动前主动退出，要求失败 Receipt、无输出和 Core `preserve`，不能算扫描成功。成功场景要求 produced Receipt、匹配的覆盖信息和 Core `proceed`。Qoder 停在隔离配置的登录边界，真实 hook 数据和完整检查链路仍未验收。

本机 Codex 默认 read-only sandbox 因 bubblewrap loopback 权限错误失败。通过的运行显式使用 `--sandbox danger-full-access`，仅执行脚本固定的 `printf`。本次没有验收 sandbox 保证，脚本也不会自动降级。

准备固定版本的 Provider 源码及 Python 依赖后，在 `src/aw` 执行以下命令，替换其中两处 Provider 绝对路径：

```bash
python3 tests/native_smoke.py \
  --host codex --case sensitive --sandbox danger-full-access \
  --provider-version 0.11.0 \
  --hook-bin "$PWD/target/debug/aw-hook-cli" \
  --provider-python /absolute/provider-checkout/src/agent-sec-core/agent-sec-cli/.venv/bin/python \
  --provider-source /absolute/provider-checkout/src/agent-sec-core/agent-sec-cli/src \
  --output-dir "$PWD/target/codex-sensitive-run"
```

其他场景改用 `clean` 或 `provider-failure`，每次选择新的输出目录。隔离登录探测使用 `--host qoder`，省略 sandbox 选项。每次运行在 `lifecycle.json` 中记录命令、PID、端口、日志和停止命令；最终 `result.json` 明确区分 passed、failed 和 blocked。模型请求只包含合成测试对话。测试结束后删除独立 Agent 配置目录，日志、配置、Receipt 和 Journal 保留在指定输出目录供核验。删除该目录即可清理其证据，或用 `cargo clean` 删除全部 AW 构建与测试产物。本次没有验证干净环境部署。
