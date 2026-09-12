# AW

[English](README.md)

AW 为能力调用提供版本化合同、离线校验和可嵌入的 Core 编排。`aw-contracts` 检查数据结构及记录间关系；`aw-core` 通过调用方提供的 Host 执行固定计划，并持久记录执行事实。AW 没有独立服务进程，原生 Agent 控制和最终工具执行仍由接入方负责。

当前接口仍处于实验阶段。默认检查使用合成记录、冻结原生输出及真实本地协议测试进程；真实 Agent 和原生扫描器验收单独执行。

## 运行检查

通过 rustup 准备 Rust，并安装 Python 3 和 Node.js。Rust、rustfmt 和 Clippy
由 [rust-toolchain.toml](rust-toolchain.toml) 固定。在仓库根目录运行：

```bash
python3 src/aw/scripts/check.py
```

入口依次运行 CI 行为测试、格式检查、Clippy、完整的 locked workspace 测试、
Python/JavaScript 摘要向量和 rustdoc。缺少工具、合同、计划、Core 执行、Journal 、adapter profile/native/bridge 、SecCore pii、共享进程、Tokenless projection/Core、原生 Host 或 hook CLI 测试目标为空或全部
ignored、向量错误及命令失败均返回非零。每条命令都有超时限制，失败或中断时
回收其子进程组。日志标明失败命令，可在 `src/aw` 单独运行对应命令定位问题。

这些检查可由普通用户运行，无需启动 Agent 或登录服务。Cargo 会下载尚未缓存的
依赖，Schema 校验只读取随包资源。检查入口要求 Linux；contracts 和 adapters 仍可移植；原生 Host 和 hook 执行要求 Linux。本门禁
不认证其他操作系统或最低支持版本。

[AW CI](../../.github/workflows/aw-ci.yml) 响应分支 push、pull request、merge group
和手动触发，校验候选提交；PR 校验合成的 merge 结果。无关变化明确返回 no-op；
范围判定错误、意外跳过或受测提交不一致均使 `AW / required` 失败。仓库管理员
需要在分支保护中选择该检查才能强制执行。工作流取消不代表门禁通过。

CI 使用 Ubuntu 24.04 x86_64、Python 3.12.3 和 Node.js 24.15.0。本地验证另使用
Linux ARM64、相同的运行时版本及固定 Rust 工具链。

## 嵌入 Core

`aw-core` 提供 `Core::prepare`、`Core::execute`、可信 Host/Clock/Journal 端口，
以及支持持久写入的 Linux `FileJournal`。准备阶段在调用任何 Provider 前检查完整
计划；执行阶段先记录调用再分发，Journal 确认后才返回终态结果。失败或中断的事件
仍保留占用记录，不自动重试或恢复。

所有权、取消、失败和接入约束见 [Core 执行与存储](docs/design/core-execution_zh.md)。
测试使用合成 Host；该 crate 尚未连接生产 Provider，也不证明原生宿主已采用结果。
统一检查强制八个 crate 的依赖边界及 Rust 文件 700 行上限（600 行提醒）；现有
合同校验文件单独保持 711 行非增长上限。这些检查辅助代码评审，不替代运行时验收。

## 原生 adapter 嵌入

`aw-adapters` 为六种固定原生 profile 捕获受支持的工具文本，并将可观测 ID 与调用方认证的 runtime 上下文核对。它通过同一个 Core 执行 post-tool 内容/代码检查，保留原始 payload。每个 Adapter 实例只在初始化时验证 profile。

受支持字段和责任划分见[原生 adapters](docs/design/native-adapters_zh.md)。此库不安装 hooks、不交付替换结果，也不授予最终 dispatch 权限。支持 pre-tool 捕获；pre-tool 执行需要单独验证的 final guard，当前 profile 会拒绝。六种映射有合成测试覆盖，不代表六个真实 Agent 已完成接入。

## SecCore 协议适配

`aw-sec-core` 将 post-tool 内容检查映射到正式 `scan-pii` 原生协议，保留用户规则与 middleware 语义，并验证原生输出。它不依赖安全引擎的实现语言。该库不启动进程或生成 Host 回执，真实调用仍由接入方负责。详见 [SecCore 原生协议适配](docs/design/sec-core-adapter_zh.md)，包括真实 CLI 黄金向量的来源、重生成与兼容验收。

## 原生 Host 与 hook CLI

`aw-sec-host` 调用已准入的 SecCore CLI，提供 stdin/stdout/stderr 限额、版本与指定文件固定、
取消以及自有进程组回收。`aw-hook-cli` 串起 Qoder/Codex `PostToolUse` 捕获、Core、Host
和已有 Journal，保留工具结果并返回观察提示；不授予安全批准，也不证明原生采用。

在仓库根目录构建实验性 Linux 入口：

```bash
cargo build --manifest-path src/aw/Cargo.toml --locked -p aw-hook-cli
src/aw/target/debug/aw-hook-cli --help
```

可信 launcher 提供私有配置和存活 Agent 身份。Qoder 限于显式绑定的单轮调用。
不自动安装 hooks，已有安全插件仍需保留。详见 [CLI 与配置参考](../../docs/user-guide/zh/user-entrypoint/aw.md)
及 [Host 架构](docs/design/sec-core-host_zh.md)。

## Tokenless 投影嵌入

`aw-tokenless-host` 通过已准入的原生 Tokenless CLI 为 Core 生成投影候选，与 SecCore
共享有界的 `aw-host-process`，仅引入原生纯协议 crate。计划选择 preserve 策略时，
失败或无收益保留原文。候选要求显式接受 `unrecoverable`；生成不证明采用或模型收益。
profile、配置及显式原生验收见 [Tokenless Host](docs/design/tokenless-host_zh.md)。

## 源码参考

- [已注册的 Schema](schemas/)与[合成输入输出样例](tests/fixtures/contracts.json)
- [公共 API](src/lib.rs)、[记录校验](src/validation.rs)与[计划校验](src/orchestration.rs)
- [编码测试](tests/canonical.rs)、[Schema 测试](tests/schemas.rs)、
  [记录测试](tests/contracts.rs)与[计划测试](tests/orchestration.rs)

Registry 包含 21 个 Schema 资源。`crates/aw-contracts/schemas/` 中的 8 份 v1 文件仅作参考，未注册到当前库。调用方需要匹配 Schema ID 和摘要，当前没有自动版本转换。

收到数据后，先用 `canonical::parse` 严格解析字节，再检查结构。结构检查通过不代表记录之间的关系正确，也不授予执行权限。计划级检查的用法见公共 API 文档，证据认证和实际动作仍由调用方负责。
