# AW 组会演示：检查、压缩与已验证的历史采用

[English](../../en/token-saving/aw-demo.md)

在真实仓库里向 Qoder 输入任务、连续追问，同时在 Herdr 查看 SecCore 检查、Tokenless 压缩和已验证的历史采用。完成一次 setup 后即可打开交互会话；也提供固定合成数据演示，方便组会彩排。

本指南同时提供真实项目多轮交互入口 `session.py` 和固定单轮演示入口 `demo.py`。发布版通常通过 `anolisa install` 安装组件，Alinux 也可使用 RPM；本组合尚未发布到这两条安装路径，下面使用源码分支。Linux ARM64 已实测；Linux x86_64 有固定 Herdr 制品，但尚未实机验收。需要可登录且能调用模型的 Qoder 账号。

## 1. 会前准备

准备 Linux、Git、C 编译器、Python 3、Rust/Cargo、uv 和 Qoder CLI。下载阶段需要访问 GitHub、Rust crates、Python 包源和 Qoder。以下基础软件安装步骤仅适用于尚未配置工具链的 Ubuntu 24.04；已有工具可跳过，不需要重装。

```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config git curl ca-certificates python3
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.97.1 --profile minimal
curl -LsSf https://astral.sh/uv/install.sh | sh
curl -fsSL https://qoder.com/install | bash -s -- --version 1.1.47
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
rustc --version
uv --version
qodercli --version
qodercli login
```

Rust 1.97.1、Qoder CLI 1.1.47 是本次验收版本；Provider 的 Python 3.11.6 由 setup 独立安装，无需替换系统 Python。Qoder 的[官方安装说明](https://docs.qoder.com/cli/installation)提供其他安装方式；使用 npm 时要求 Node.js 20 及以上，可执行 `npm install -g @qoder-ai/qodercli@1.1.47`。

沿用当前用户的 Qoder 登录，不复制认证。当前入口要求用户级 Qoder hooks 和 plugins 为空；如日常账号装有集成，请在独立演示 OS 账号登录并运行，不要为演示卸载日常安全插件。首次登录后运行一次 `qodercli`，看到正常输入界面后退出，确保 `~/.qoder/settings.json` 已建立。演示使用默认 `~/.qoder`，不支持 `QODER_CONFIG_DIR` 覆盖。

## 2. 从空目录 clone、编译和 setup

在你选定的空目录执行下列命令。无需另一个本地 Provider 工作树，也无需填写绝对路径。

```bash
git clone --single-branch --branch feat/aw/tokenless-herdr https://github.com/kongche-jbw/anolisa.git anolisa-demo
cd anolisa-demo
python3 src/aw/scripts/demo.py setup
python3 src/aw/scripts/demo.py providers
python3 src/aw/scripts/demo.py doctor
```

`setup` 完成以下步骤，每步有超时和独立日志：

1. 获取 SecCore 提交 `5ebfc0b3905fa2f5f74aff2da4aec2b3be639647`，只展开原生 Provider 所在的 Python 项目。
2. 用该提交的 `uv.lock` 安装 Python 3.11.6 及运行依赖。只使用 scanner 原生入口，不构建或安装 SecCore 守护进程。
3. 从本次 clone 编译 Tokenless 0.8.0，以及 `aw-hook-cli`、`aw-view-cli`、`aw-adoption-cli`。
4. 下载未修改的 Herdr v0.9.0 并校验固定 SHA-256，保留上游许可证。
5. 核验 Provider 导入、版本、协议与构建产物。

生成内容集中在 `src/aw/target/demo/`：`sec-core/` 是固定源码与 venv，`python/` 与 `uv-cache/` 是 Python 工具缓存，`build/` 是 Rust 产物，`herdr/` 是面板，`setup-logs/` 是构建日志，`runs/` 是每次演示证据。再次执行 setup 会复用依赖并增量构建；发现 SecCore 已修改或 Herdr 摘要不匹配时失败并指出路径。

`doctor` 还会检查 Qoder 版本、登录状态和集成冲突，但不发起模型请求。模型访问是否可用由下一步真实彩排确认。彩排与正式演示都调用真实模型，可能产生账号用量。

## 3. 在真实仓库中自由多轮交互

完成上面的 setup 后，在 cosh-shell 的 Bash 提示符进入要工作的仓库目录，激活一次当前 shell 的入口，然后直接输入 `qoder`。`AW_CHECKOUT` 指向包含 AW 代码的 clone，`$PWD` 是你的真实项目：

```bash
AW_CHECKOUT=/path/to/anolisa-demo
source "$AW_CHECKOUT/src/aw/scripts/activate.sh" --allow-unrecoverable
qoder
```

也可以在 AW clone 根目录运行：

```bash
python3 src/aw/scripts/session.py --workspace "$PWD" --allow-unrecoverable
```

Qoder 会在指定目录打开。Herdr 原生客户端直接继承当前终端，键盘输入和窗口大小由 Herdr 自己处理。启动器负责进程生命周期，不读取或转发键盘。直接输入任务，连续追问、滚动、取消当前生成、确认工具权限。不会自动发送提示词，也不会生成 fixture。仓库的源码、Git 分支与已有配置文件不由启动器修改；你授权 Qoder 执行的实际开发操作仍可能修改项目。首次遇到目录信任提示时由你决定，启动器不自动确认。

可按下面的顺序体验当前 ANOLISA 仓库：

1. 输入“先用 Bash 查看当前仓库的 Git 状态和主要源码目录，解释各模块用途，先不要修改文件。”
2. 继续输入“用 Bash 执行 `cargo metadata --manifest-path src/tokenless/Cargo.toml --no-deps --format-version 1`，分析 Tokenless 的 crate 依赖关系。”
3. 根据回答追问一个真实问题，例如“哪一层负责选择压缩器？请查看代码说明调用路径。”
4. 查看 Herdr 的累计调用、`history adopted` 与节省字节。不是每条输出都能压缩；短输出、无法处理的格式和失败命令不会被计为节省。

`activate.sh` 只为当前 Bash 定义 AW 的 `qoder` 函数，不修改启动文件或覆盖磁盘上的 Qoder 程序。退出 Qoder/Herdr 后回到原 shell，可再次输入 `qoder`。撤销本 shell 的接入：

```bash
unset -f qoder
unset _AW_SESSION_ENTRY
```

侧栏使用显式高对比度文字，分别显示 Bash 结果数、SecCore/Tokenless 版本、各自调用数、最近结果，以及独立历史采用和节省字节。一个 Bash 结果可能调用两个 Provider，因此不会再把两个 Provider 的调用总数当作 Bash 次数。

按 `Ctrl+B`，松开后按 `p`，打开 **AW PROVIDERS** 详情弹窗：`1` 查看实际可执行路径、SecCore 源码目录、原生协议、配置与执行版本、Manifest 摘要和累计统计；`2` 查看最近 40 次 Provider 调用的工具 ID、结果、耗时、输入/候选字节数及实际压缩操作。上下键滚动，`q` 关闭弹窗回到 Qoder。

例如 Tokenless 原生返回 `no_savings`，显示 `preserved: no savings`；超时显示失败代码。旧证据没有保存原生原因时明确显示 `native reason not recorded`，不从 `bypassed` 猜测原因。详情来自当前会话 Rust 校验通过的事件；出现验证错误或会话切换会清除旧详情。源码路径和 Provider 结果仅在本地展示。

当前 AW 接入主 Agent 的 Bash 后置输出。Read、Edit 等其他工具仍由 Qoder 正常使用，侧栏以 `Bash only` 明确范围。Bash 失败、超过原生捕获限制或无法验证的结果显示为 `unverified`，不增加已采用节省。等待权限确认或原生历史落盘期间显示 `pending`，不提前计入采用。这里的 SecCore 是后置内容检查，不提供前置命令拦截或 OS 防护。

普通追问保留当前会话的累计值。要从零开始，请退出后重新运行启动命令。固定 Herdr v0.9.0 不支持替换 Qoder 会话绑定；若在 Qoder 内输入 `/new` 或切换原生会话，AW 会清除侧栏旧统计并提示重新启动，本进程后续工具保留原生行为、不再执行 AW 检查或压缩。旧证据保留。默认最长运行一小时；用 `--duration 7200` 可设为两小时，允许范围为 60～14400 秒。

退出时按 `Ctrl+B`，松开后按 `q`；这会结束此入口管理的 Qoder、观察器与 Herdr。Qoder 进程正常退出时，入口也会结束。Qoder 的原生历史、你修改的项目文件和你选择保存的信任设置会保留。启动时和退出后都会打印 AW 证据目录 `src/aw/target/sessions/<UUID>/`，权限为 0700，包含本次 Bash 原文、采用记录和日志；按实际数据管理要求保管它。删除单次 AW 证据不会删除 Qoder 历史或回滚代码。

```bash
rm -rf -- "$AW_CHECKOUT/src/aw/target/sessions/<UUID>"
```

`--workspace` 默认当前目录；`--provider-dir` 默认 AW clone 的 `src/aw/providers/`，可选显式可信 manifest 目录。现有用户或项目 hook/plugin 共存仍未验收，启动时发现已配置 hook 或已安装 Qoder plugin 会明确退出。使用默认 `~/.qoder` 登录配置，不支持 `QODER_CONFIG_DIR`。恢复旧会话、子 Agent、自定义 cwd 切换和其他原生工具投影不在当前绑定合同内；无法绑定时保留原工具行为并显示未验证。

多轮入口实际使用 `scripts/session.py` 启动 Herdr/Qoder，`session_hooks.py` 从原生 `UserPromptSubmit` 生成启动器轮次，并在 `PreToolUse` 为每个工具固定轮次；`PostToolUse` 再调用同一 Rust `aw-hook-cli`。独立进程 `session_observer.py` 等待完整历史行，通过 `aw-adoption-cli`、`aw-view-cli` 验证后发布统计。观察器独立运行，验证不会阻塞 Herdr 原生输入。Provider、AW Core 和 Schema 沿用 setup 的实现；运行路径不经过测试脚本。原生事件依据见 [Qoder hooks](https://docs.qoder.com/cli/hooks)。

## 4. 固定场景彩排与现场演示

先做一次完整彩排；即使使用 `--headless`，压缩场景仍会启动真实 Qoder 和 Herdr，并验证实际 TUI 内容，只是不把画面输出到当前终端。

```bash
python3 src/aw/scripts/demo.py run --headless --allow-unrecoverable
```

现场将终端设为至少 120 列 × 40 行，然后执行：

```bash
python3 src/aw/scripts/demo.py run --allow-unrecoverable --hold-seconds 30
```

会显示真实 Herdr TUI 内的 Qoder。脚本自动信任本次新建的合成工作目录，提交只执行 `cat fixture.json` 的提示词。验证完成后侧栏保持 30 秒供讲解，然后退出并清理。无需手动输入提示词；这是自动单轮演示，不是任意多轮聊天入口。`--allow-unrecoverable` 显式允许替换本次合成工具结果。

建议按下面的顺序讲解，总计约三分钟：

| 阶段 | 画面或操作 | 可讲的内容 |
| --- | --- | --- |
| 启动前 | `providers` 列出两个 Provider | 安装路径由配置文件管理，启动时默认发现；无需给每个 hook 拼路径 |
| 执行工具 | Qoder 执行一次 `cat fixture.json` | Agent 使用原生工具，后置 hook 把结果交给 AW |
| 检查与压缩 | SecCore、Tokenless 侧栏 | AW 按计划执行检查与压缩，记录各 Provider 的执行结果 |
| 采用确认 | `history adopted` 和负的字节数 | 独立观察器核对 Qoder 本地历史；候选生成后还需要采用确认才累计节省 |
| 退出后 | `Demo passed` 和证据路径 | 每次演示有独立会话、记录和清理结果，可以复核 |

历史基线为 4459 B → 3107 B，节省 1352 B，采用 1 次。以当次输出为准；这里统计字节，不是模型 Token 或账单金额。`history adopted` 表示本地历史采用，不宣称远端模型已经消费。SecCore 此处执行内容检查，不代表 OS 隔离或前置阻断。

实现流程：

```mermaid
sequenceDiagram
    participant Q as Qoder
    participant A as AW hook / Core
    participant S as SecCore
    participant T as Tokenless
    participant O as 独立采用观察器
    participant H as Herdr
    Q->>A: Bash 成功 stdout
    A->>S: 内容检查
    S-->>A: 检查结果
    A->>T: 压缩候选
    T-->>A: 更小的文本
    A-->>Q: 原生工具结果替换
    Q->>Q: 写入本地历史
    O->>Q: 读取匹配工具调用的历史
    O->>A: 验证计划、执行记录与采用
    A-->>H: 当前会话的已验证摘要
```

## 5. 无收益与故障演示

这两个补充场景运行非交互 Qoder，在终端输出结果，不启动 Herdr 组合验收。每次自动生成新目录，不需要手工改路径。

```bash
python3 src/aw/scripts/demo.py run --headless --allow-unrecoverable --case no-gain
python3 src/aw/scripts/demo.py run --headless --allow-unrecoverable --case provider-failure
```

`no-gain` 应保留原文，采用与节省均为零。`provider-failure` 让 Tokenless 在压缩前退出，应出现 failed Receipt、Core preserve、原文保留和零节省。这里演示的是可观察的失败处理，不是把失败计为优化成功。

## 6. Provider 放置与发现规则

默认读取 `src/aw/providers/*.json`，与运行命令时所在目录无关。当前支持一个 `sec-core` 和一个 `tokenless`，按 ID 选择，目录顺序不改变 Core 的执行顺序。缺失、重复 ID、不支持的种类、版本或协议都会显式失败。`providers` 仅读取配置；`doctor` 才检查并执行版本/导入探测。

| 字段 | 含义 |
| --- | --- |
| `id` | 当前固定为 `sec-core` 或 `tokenless` |
| `kind` | 分别为 `security`、`projection` |
| `version` | 当前分别为 `0.11.0`、`0.8.0` |
| `native_protocol` | 分别为 `1`、`2`；不等于 AW 能力 Schema 版本 |
| `program` | 可执行文件，绝对路径或相对当前 manifest 的路径 |
| `source` | 仅 SecCore 使用的 Python 源码目录，同样相对 manifest 解析 |

可以把两个 manifest 放到自己信任的目录，用全局参数 `--provider-dir /absolute/providers` 选择，再执行 `providers`、`doctor` 或 `run`。不要从 Agent 的工作目录自动递归发现并执行程序。`setup` 始终准备仓库默认布局，不替自定义 manifest 安装依赖。

发现功能属于本次演示启动器；读取文件后仍会生成显式、固定的 Provider 调用配置交给现有 Host。它不是通用 Provider 安装器或 Core 的热加载协议，不会自动安装原生插件。

## 7. 排障、证据与清理

| 症状 | 处理 |
| --- | --- |
| setup 失败 | 打开输出中的 `setup-logs/step-*.log`，解决网络/编译依赖后重跑；不会自动切换系统旧版 Tokenless |
| Qoder 版本不符 | 在演示账号准备 1.1.47；演示项目禁用自动更新，不修改用户全局设置 |
| 登录或模型失败 | `qodercli login` 后确认交互 Qoder 可正常使用，再重新运行；doctor 不保证模型请求成功 |
| 用户 hook/plugin 冲突 | 使用独立演示账号；脚本不会自动关闭现有集成 |
| 非终端环境 | 固定演示用 `demo.py run --headless`；`session.py` 需要真实终端 |
| 交互统计未验证 | 查看 `target/sessions/<UUID>/errors/`、`calls/*/hook.stderr.log`、`calls/*/verification-error.log` 和 `herdr.log`；超出范围的工具不计节省 |
| 演示失败 | 查看所打印目录的 `herdr/command.log`、`agent/result.json` 和 `agent/lifecycle.json`，不要将失败目录当作成功证据 |

以下证据与自动删除规则适用于固定 `demo.py run`；真实 `session.py` 的保留规则见第 3 节。

固定压缩场景的 `herdr/result.json` 应为 `passed`，并包含 `real_sidebar_render: passed`、正的 `saved_bytes`。`agent/adoption-verified.json` 与 `agent/evidence/` 保留采用和执行证据；`herdr/screen.ansi` 保存实际画面输出。它是终端捕获文件，不是交互式回放或录屏视频。

每次 `run` 结束时停止自己创建的 Qoder、Herdr 客户端和服务，删除临时 home、专属 Qoder 会话和本次添加的信任条目。按 Ctrl+C 提前结束也走清理路径。中断 setup 会停止它自己的进程组，保留日志和增量构建材料。若进程被强制 SIGKILL 或机器断电，则需检查记录中的 PID 与代次，再使用 `launcher.json`、`agent/lifecycle.json`、`herdr/ownership.json` 的准确停止命令处理残留。

日志和证据有意保留。每次成功退出都会打印删除该次记录的精确命令。确认无需保留任何演示构建、缓存和证据后，在仓库根目录清理全部本地演示材料：

```bash
rm -rf -- src/aw/target/demo
```

此命令不卸载会前安装的 Rust、uv、Qoder，也不删除账号认证。详细的证据合同和早期验收记录见[实现与复现说明](../../../../src/aw/docs/design/tokenless-herdr-acceptance_zh.md)。
