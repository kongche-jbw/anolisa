# Tokenless 与 Herdr 组合验收

[English](tokenless-herdr-acceptance.md)

本次在 Qoder/Codex 检查基线上继续接入：Qoder 在同一 AW 计划中调用 SecCore 和 Tokenless，独立观察器核验原生历史中的工具结果，Herdr 展示当前会话的已验证数据。公开 Schema 和 Core 实现保持不变。

## 从新 clone 开始的组会演示

已提供 [完整 setup、演示与清理流程](../../../../docs/user-guide/zh/token-saving/aw-demo.md)。在仓库根目录执行：

```bash
python3 src/aw/scripts/demo.py setup
python3 src/aw/scripts/demo.py providers
python3 src/aw/scripts/demo.py doctor
python3 src/aw/scripts/demo.py run --allow-unrecoverable --hold-seconds 30
```

默认发现 `src/aw/providers/*.json`，相对路径以 manifest 位置解析。Setup 自动准备固定 SecCore 提交及 Python 环境、本分支 Tokenless/AW 二进制和固定 Herdr 制品；不再要求另一个本地工作树。发现只在启动器完成，Host 仍接收显式固定配置，Schema 与 Core 不变。未知、缺失、重复或版本/协议不符的 Provider 明确失败，不扫描 Agent 工作目录，也不安装全局插件。

现场入口镜像真实 Herdr TUI，在独立采用验证后持续刷新侧栏，按选定时长保留画面，然后清理专属会话。提示词用代码标记限定 `cat fixture.json`，避免标点被误当成命令参数。单轮身份、插件共存限制和采用证据边界仍适用。以下低层命令保留用于诊断；新 clone 应优先使用上述入口。

## 构建与运行

本次核验环境为 Linux ARM64。源码 Tokenless 为 0.8.0，使用 native protocol v2；系统原有的 0.7.0 二进制不适用。Herdr 固定为官方未经修改的 v0.9.0。Qoder 验收使用 1.1.47，原位沿用已有登录。SecCore 仍依赖显式选定的 0.11.0 原生 Provider 源码及 Python 3.11.6 环境，详见[检查接入说明](qoder-codex-inspection_zh.md)。

在仓库根目录执行：

```bash
cargo build --manifest-path src/tokenless/Cargo.toml -p tokenless-cli --locked
cd src/aw
cargo build -p aw-hook-cli --bins --locked
python3 integrations/herdr/fetch.py target/herdr-integration/bin
```

在 `src/aw` 下执行。替换两处 Provider 绝对路径，每次选用尚不存在的输出目录：

```bash
PYTHONDONTWRITEBYTECODE=1 timeout 150 python3 tests/tokenless_smoke.py \
  --case compressed \
  --hook-bin "$PWD/target/debug/aw-hook-cli" \
  --view-bin "$PWD/target/debug/aw-view-cli" \
  --adoption-bin "$PWD/target/debug/aw-adoption-cli" \
  --tokenless-bin "$PWD/../tokenless/target/debug/tokenless" \
  --provider-python /absolute/provider-checkout/src/agent-sec-core/agent-sec-cli/.venv/bin/python \
  --provider-source /absolute/provider-checkout/src/agent-sec-core/agent-sec-cli/src \
  --output-dir "$PWD/target/tokenless-run"
```

场景可选 `compressed`、`no-gain` 和 `provider-failure`。每次只执行一次合成数据的只读 `cat fixture.json`；故障场景让 Tokenless 子进程在压缩前主动退出。脚本显式允许不可恢复替换；发现已安装 Qoder 插件或用户 hook 时会拒绝运行，不会静默关闭原有安全或压缩集成。非交互测试仅加载项目设置；交互测试使用默认设置来源，并显式选用已有登录可用的 `auto` 模型。两者都不使用系统 Tokenless 或其全局统计。

要在真实 Herdr 内组合验收，将上面的命令保存为 JSON argv 数组，使用绝对路径和新的 `--output-dir`，然后执行：

```bash
PYTHONDONTWRITEBYTECODE=1 timeout 210 python3 integrations/herdr/live_smoke.py \
  target/herdr-integration/bin/herdr \
  --output target/herdr-integration/live-run \
  --command-json /absolute/owned-command.json
```

组合入口启动独立的 Herdr 服务和 TUI，正常进入 Qoder，只信任本次新建的测试目录，再通过原生终端按键提交一条提示词。本机交互测试中，限定 `--setting-sources project` 后 `/hooks` 显示项目 hook 为零；因此采用默认来源，并先确认用户 hook 和插件为空。测试目录单独初始化 Git 根，正常进入界面后再提交提示词，不依赖 `-i` 初始提示路径。Agent PID 与启动代次必须对应实际 pane 前台进程；发布计数前，启动器报告本次捕获的原生会话 ID。

## 证据与重点评审

- `native-event.json` 绑定成功 Bash stdout、会话和工具调用 ID。
- `evidence/<event>.json` 保存计划、调用元数据、Receipt、候选和 Core Journal 确认。采用验证时从捕获的原生事件重建输入内容；hook 准备交付仍单独记为 `prepared_for_return`。
- `adoption-journal/` 与 `<event>.adoption.json` 只保留匹配的两条原生历史行及已有 AW 采用合同。原测试会话删除后，`verify` 仍可将它们与完整计划和执行记录关联。这里信任本地记录器与存储所有者，不提供抵抗同用户恶意进程重写的签名保证。
- `view-binding.json` 选定原生会话。可选的 `adoption_bindings` 按事件键索引各自的可信采用绑定文件，不能用一个调用的原生输入核验另一个调用。只有独立采用验证通过，才累计节省；Journal 损坏或绑定不符会明确失败，其他会话被排除。
- `herdr-view.json`、`herdr-metadata.json` 和实际 `screen.ansi` 分别对应校验器输出、原生 metadata 与渲染结果。侧栏写作 `history adopted`，不表示模型已消费。混合失败或绕过不会抹掉已有采用与节省。

当前捕获入口仍使用显式限定的 Qoder 单轮身份。通用多轮启动身份、原生插件共存、COSH 自动生命周期接入、Checkpoint 操作、OS 隔离和干净机器打包需要后续处理。Codex 保留检查路径，配置 Tokenless 投影时明确拒绝；其 hook 没有这里使用的 Qoder 替换合同。ARM64 已核验；x86_64 Herdr 制品虽已固定，本次未运行。

各入口在 `lifecycle.json` 或 `ownership.json` 中记录命令、版本、PID 与启动代次、路径和停止命令。成功清理只删除本次新 UUID 会话、同名状态目录、临时 home，以及测试目录的信任条目。认证不复制、不删除，日志与合成证据保留在所选目录。共享 VM 和现有 Herdr 不会被重启。若不再保留本工作树的生成材料，可分别对 AW 与 Tokenless 的 Cargo manifest 执行 `cargo clean`；先保存需要的证据。

## 本次核验结果

新增 setup/demo 入口在 Linux ARM64 上通过独立 clone 验证（已有系统工具链与 Qoder 登录，变更文件带入新 clone；不是全新 OS 安装验收）。固定 Provider 源码重新从远端获取，Python 环境与 Rust 产物在该 clone 独立生成。真实 Herdr 终端展示、无收益、Provider 故障及验证后中断清理均通过；144 项 Rust 测试、15 项 Herdr Python 测试、7 项启动器测试、fmt、Clippy、文档构建与跨语言摘要检查通过。新证据在 `target/demo/`，不与下面的早期记录混用。

| 场景 | 实际结果 |
| --- | --- |
| Qoder + Herdr 组合 | 4459 B → 3107 B；本地历史采用 1 次，节省 1352 B；实际 metadata 和 TUI 数值一致 |
| Qoder 无节省 | 保留原文；采用 0 次、节省 0 B |
| Qoder Tokenless 故障 | failed Receipt、Core preserve；原生历史保留原文，节省 0 B |
| Codex 检查回归 | 实际 CLI → SecCore 通过；第二次脚本化模型请求仍含原工具结果 |

Rust 共 144 项、Herdr Python 共 15 项通过；fmt、Clippy、文档构建及跨语言摘要检查通过。组合证据位于 `target/tokenless-herdr/qoder-combined/` 与 `target/herdr-integration/combined/`；无节省、故障和 Codex 分别位于同级的 `qoder-no-gain-verified/`、`qoder-provider-failure/`、`codex-regression/`。较早的诊断目录保留用于定位交互配置和绑定问题，不作为成功证据。当前所有测试进程均已结束。
