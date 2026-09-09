# 固定版本 Herdr 展示接入

[English](herdr-integration.md)

这次接入使用未修改的 Herdr v0.9.0，通过原生 pane metadata 和侧栏配置展示
经过校验的 AW 结果。Herdr 不调用 Provider，不校验 Receipt，不安装 Agent
插件，也不决定安全策略。

## 版本与获取方式

[`upstream.json`](../../integrations/herdr/upstream.json) 固定官方正式版本
`v0.9.0`、源码提交 `b99002ac99b09e00b4ca692436cb15a6b0d676f1`、Linux ARM64
和 x86_64 制品摘要，以及 Apache-2.0 许可摘要。旧 PoC 使用的源码快照不是
这个版本。本次没有携带上游补丁，也没有复制上游源码进仓库。

获取脚本复用文件前会校验摘要，发现不一致就拒绝；上游许可证与可执行文件
保存在一起。脚本只下载当前架构的发布制品，不能据此宣称源码构建可复现。
升级需要显式更新固定信息并重跑兼容性测试，默认关闭自动更新。

在 `src/aw` 中执行。获取制品需要 Python 3 和网络：

```bash
timeout 120 python3 integrations/herdr/fetch.py target/herdr-integration/bin
PYTHONDONTWRITEBYTECODE=1 timeout 60 python3 integrations/herdr/smoke.py \
  target/herdr-integration/bin/herdr --output target/herdr-integration/smoke
PYTHONDONTWRITEBYTECODE=1 timeout 30 python3 -m unittest discover \
  -s integrations/herdr/tests -v
```

## 证据与展示的边界

启动器向 `aw-view-cli` 提供可信绑定文件，其中包含 runtime、完整会话 scope、
Agent PID、Linux 进程启动 ticks，以及 Journal 和 evidence 的绝对路径。Rust
校验器负责检查输入、Receipt 和 Journal 的关联。桥接脚本只消费校验后的摘要，
不读取旧 Tokenless 统计，也不会把生成候选推断成实际采用。

调用校验器前后，桥接脚本都会通过原生 `pane.get` 检查会话，通过
`pane.process_info` 检查进程。Agent PID 必须是 pane 的 shell 或终端前台进程组
成员；可以经过 wrapper，但无关进程或脱离前台组的进程会被拒绝。原生会话必须
已经由 Herdr 观察到，或由可信启动器报告。显示名称不能代替会话绑定，执行
`/new` 后需要新的可信绑定。

内部 view 使用 `format: 1`，要求 scope 完全匹配、`runtime_alive: true` 和
`verification: journal_verified`。Provider 用 `kind: security` 或 `projection`
标明展示类别。只有当前绑定校验成功时，空证据才表示零次调用。证据损坏、
runtime 已退出或会话不匹配时，侧栏清除原有统计，显示 `AW unknown`。

采用状态为 `not_observed` 时，已采用次数和节省字节必须都为零。存在独立校验
通过的 `local_history` 证据时，侧栏写作 `history adopted`，只表示本地历史采用，
不表示候选已进入模型请求。view 分别保留调用、候选、失败和绕过次数。同一会话
出现混合结果时，侧栏并列保留历史采用、节省字节、失败和绕过次数，最后一行也
显示失败与绕过总数，避免 Provider 行过长被截断后遗漏这些结果。没有失败或
绕过时，最后一行显示退出提示；采用未知时仍保留候选次数。默认画面不展示输入
内容、摘要或私有路径。

精简布局支持一个安全 Provider 和一个投影 Provider；不支持的类别或同类多个
Provider 会明确失败。这是当前展示限制，不是 AW Schema 的能力限制。

固定版本的原生会话 API 只接受预注册 source。Qoder 启动器使用 `source=herdr:qodercli`、`agent=qodercli`、递增序号和 `session_start_source=startup`；任意自定义 source 的报告即使 RPC 返回成功，也不会建立会话绑定。这个名称用于上游接口兼容，实际身份仍由本次启动器、PID 代次和原生事件校验，不充当安全认证。

## 向已托管 pane 发布

使用启动器选定的专用 Herdr socket 和准确的 pane ID：

```bash
python3 integrations/herdr/bridge.py \
  --socket /path/to/isolated/api.sock --pane pane-id \
  --verifier "$PWD/target/debug/aw-view-cli" \
  --binding /path/to/launcher-owned-binding.json --duration 180
```

这些路径是占位示例，并非已部署目录。watch 时长限制为 1～3600 秒，省略
`--duration` 则只发布一次。固定 reporter source 为 `anolisa.aw`，序号递增，
metadata 的 TTL 为五秒。校验失败会清除旧计数；watch 正常退出时主动清除自己
的 token，异常退出后由 TTL 使其消失。一个 pane 只运行一个 reporter。

[`config.toml`](../../integrations/herdr/config.toml) 提供五行：原生 Agent 状态、
AW 校验状态、SecCore、Tokenless，以及采用范围和退出提示。SecCore 调用只是
检查记录，不表示 OS 防护。面板不宣称 AgentSight 或 Checkpoint 已启用。
Provider 配置仍由 AW 启动器负责，面板不热安装原生插件。使用现有 opt-in AW
hook 配置启动 Agent 后，正常调用工具即可。按 `Ctrl+B`，松开后按 `q` 退出
Herdr 客户端，Agent 继续运行。

## 隔离与验收范围

smoke 脚本启动固定的真实制品，使用独立 API socket、配置和状态目录，以及
不需要登录的 Bash pane。为满足 Unix socket 路径长度限制，它使用短的临时
目录。验证开始前记录命令、PID、socket 和停止命令；结束后只终止并等待自己
创建的进程组，删除该临时目录，不连接已有 Herdr，也不读取 Agent 认证。
侧栏明确写着 UI fixture，不能作为 AW 执行或采用证据。

本次 Linux ARM64 验证覆盖制品摘要、原生 metadata 写入和读回、真实 120×40 和 80×24
TUI 侧栏渲染、终端输入、键盘退出后 pane 仍存活，以及 metadata 过期。
ANSI 输出、进程归属和结果 JSON 保存在 `--output` 目录。x86_64 已固定制品，
但没有完成实机测试。弹窗行为、COSH 生命周期接入、干净 VM 部署和重新设计的
配置面板不在这次验收范围内。

`live_smoke.py` 是独立的组合验收入口。它通过 `--command-json` 读取可信 JSON
argv，在隔离的真实 Herdr pane 中运行 Tokenless 验收命令，并补充
`--interactive`、`--herdr-socket` 和 `--herdr-pane-id`。该命令需要在新的
`--output-dir` 中保存 `herdr-view.json` 和 `herdr-metadata.json`。组合入口要求
历史采用校验成功、metadata 对应，并在真实 TUI 中找到一致的节省字节文本。
它不能把无认证 UI fixture 测试转换成 AW 证据；真实 Agent 的结果单独记录。

## 上游依据

- [官方不可变 v0.9.0 发布](https://github.com/herdrdev/herdr/releases/tag/v0.9.0)
- [固定版本的 pane metadata 与进程合同](https://github.com/herdrdev/herdr/blob/b99002ac99b09e00b4ca692436cb15a6b0d676f1/src/api/schema/panes.rs)
- [固定版本的侧栏配置](https://github.com/herdrdev/herdr/blob/b99002ac99b09e00b4ca692436cb15a6b0d676f1/docs/preview/website/src/content/docs/configuration.mdx)
- [固定版本许可证](https://github.com/herdrdev/herdr/blob/b99002ac99b09e00b4ca692436cb15a6b0d676f1/LICENSE)
