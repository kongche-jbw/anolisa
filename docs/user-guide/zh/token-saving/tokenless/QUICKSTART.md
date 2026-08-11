# Tokenless 快速开始

[English](../../../en/token-saving/tokenless/QUICKSTART.md)

大约三分钟内完成 Tokenless 安装、产生一条压缩结果、接入一个 Agent，并确认一条
压缩前后的 Token 记录。Tokenless 在后台工作，不需要改变 Prompt 或日常使用
Agent 的方式。

实际节省效果取决于工作负载。工具调用密集型任务通常最明显；较短或以对话为主的
任务可能变化不大。

## 1. 安装 Tokenless

先安装 anolisa CLI，再用它安装 Tokenless：

```bash
curl -fsSL https://get.agentic-os.sh | bash
export PATH="$HOME/.local/bin:$PATH"
anolisa --version
anolisa install tokenless
tokenless --version
```

如果 `anolisa --version` 已经能正常返回，可以直接从
`anolisa install tokenless` 开始。上面的 PATH 设置会让默认安装目录在当前
Shell 中立即生效，新的登录 Shell 可能已经包含 `~/.local/bin`。

## 2. 查看第一条压缩结果

修改任何 Agent 配置前，先进行一次结果确定的响应压缩检查：

```bash
printf '%s\n' \
  '{"status":"ok","data":{"name":"demo","items":[1,2,3]},"debug":{"trace":"verbose"},"metadata":null}' \
  | tokenless compress-response

tokenless stats list --limit 1
tokenless stats summary
```

第一条命令返回的仍是合法 JSON，其中 `debug` 和 `metadata` 会被省略。随后两条
统计命令应显示一条最近记录，其 Token 估算值从压缩前到压缩后有所下降。如果输出
没有变化，请完全按照上面的示例重试；不包含可移除字段的内容会原样返回且不记录。

## 3. 将 Tokenless 接入 Agent

扫描已经安装的 Agent 框架：

```bash
anolisa adapter scan
```

复制与你所用 Agent 对应的启用命令：

| Agent | 启用命令 |
|-------|----------|
| cosh / Copilot Shell | `anolisa adapter enable tokenless cosh` |
| OpenClaw | `anolisa adapter enable tokenless openclaw` |
| Hermes | `anolisa adapter enable tokenless hermes` |
| Qoder | `anolisa adapter enable tokenless qoder` |
| Claude Code | `anolisa adapter enable tokenless claude-code` |
| Codex | `anolisa adapter enable tokenless codex` |
| Qwen Code | `anolisa adapter enable tokenless qwencode` |

只启用准备使用的 Agent，然后检查 Adapter Receipt 和组件健康状态：

```bash
anolisa adapter status tokenless
anolisa doctor tokenless
```

重启 Agent CLI 或 IDE，使其加载新的 Adapter。OpenClaw 需要显式重启 Gateway：

```bash
openclaw gateway restart
```

如果 OpenClaw 在安全检查时拒绝 Plugin，请先检查报告的风险，再按照
[OpenClaw 接入说明](framework-integration.md#2-启用一个-adapter)确认后重试。
OpenCode 当前使用单独的生命周期脚本，详见
[框架接入](framework-integration.md#opencode)。

## 4. 运行真实任务并验证节省效果

启动新的 Agent Session，并运行一次工具密集型任务。例如：

> 运行当前仓库的完整测试，只总结失败项。

Prompt 中不需要提到 Tokenless。Agent 使用一次 Shell、API 或其他受支持的工具后，
运行：

```bash
tokenless stats list --limit 5
tokenless stats summary
```

当 `anolisa adapter status tokenless` 显示 Adapter 已启用，并且 `stats list`
中出现 Token 估算值从左到右下降的记录时，首次体验即完成。

如需检查某条记录具体改变了什么，复制其 ID 后运行：

```bash
tokenless stats diff <record-id>
```

如果没有记录，可能是内容没有经过 Tokenless，或处理后没有变短。请参阅
[开启后没有产生统计记录](troubleshooting.md#启用后没有产生统计记录)。

Token 数只是在 Tokenless 已处理内容范围内的估算值，不等于模型账单的直接变化。
统计和 diff 可能包含原始工具内容；涉及敏感数据时不要分享输出。完整说明见
[效果度量](measuring-savings.md)和
[配置与数据隐私](configuration-and-privacy.md)。

## 5. 平台适配性

| 平台 | anolisa CLI 安装 |
|------|------------------|
| Linux x86_64/aarch64 | 支持 |
| macOS Apple Silicon | 支持 |
| macOS x86_64 | 暂不支持 |
| Windows 或使用 musl 的 Linux（例如 Alpine） | 暂不支持 |

本页只提供 anolisa CLI 安装路径。需要从源码构建独立 CLI 时，请参阅
[用户手册 · 从源码构建独立 CLI](user-manual.md#从源码构建独立-cli)。

## 6. 下一步

- [框架接入](framework-integration.md)：各框架的激活方法和实际行为
- [用户手册](user-manual.md)：能力边界和文档导航
- [CLI 参考](cli-reference.md)：全部子命令和参数
- [效果度量](measuring-savings.md)：统计、双跑对比和 AgentSight/SLS
- [配置与数据隐私](configuration-and-privacy.md)：开关、存储和敏感数据
- [故障排查](troubleshooting.md)：常见错误、升级和卸载
