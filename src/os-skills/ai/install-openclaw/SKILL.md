---
name: install-openclaw
description: Install and configure OpenClaw 4.26 on Linux, with Qwen as the first-class provider, Qwen Cloud / Alibaba DashScope standard endpoint, Coding Plan endpoint, DingTalk Connector, gateway startup, and troubleshooting. Use when the user asks to install OpenClaw, configure Qwen/QWEN_API_KEY, select standard or Coding Plan, set the default model, configure DingTalk, or debug OpenClaw gateway/model/channel issues.
---

# OpenClaw 4.26 安装配置指南（Qwen + 钉钉）

默认推荐安装已验证版本：`openclaw@v2026.4.26`。

OpenClaw 4.26 已将 Qwen 作为一等内置提供商处理：

- 规范 provider ID：`qwen`
- 首选环境变量：`QWEN_API_KEY`
- 兼容环境变量：`MODELSTUDIO_API_KEY`、`DASHSCOPE_API_KEY`
- 模型引用格式：`qwen/<model-id>`
- 旧版 `modelstudio/...` 仅作为兼容别名，新配置优先使用 `qwen/...`

## 目录结构

```text
install-openclaw/
├── SKILL.md
├── scripts/
│   └── configure_openclaw_dingtalk.py
└── references/
    ├── troubleshooting.md
    └── dingtalk-setup-guide.md
```

遇到问题先查 `references/troubleshooting.md`。钉钉开发者平台操作细节查 `references/dingtalk-setup-guide.md`。

## Phase 1: 前置依赖

目标环境：

| 项 | 要求 |
|---|---|
| OS | Linux / Alibaba Cloud Linux / Anolis |
| Node.js | v22+ |
| npm | 随 Node.js 安装 |
| Python | Python 3，用于配置脚本 |

检查：

```bash
node --version
npm --version
python3 --version
```

Node.js 不满足 v22 时，优先使用系统包：

```bash
dnf install -y nodejs npm
```

## Phase 2: 安装 OpenClaw 4.26

默认安装已验证版本：

```bash
npm install -g openclaw@v2026.4.26
```

国内网络慢时：

```bash
npm install -g openclaw@v2026.4.26 --registry=https://registry.npmmirror.com
```

验证：

```bash
openclaw --version
which openclaw
```

如果 `openclaw --version` 不是 `4.26` 系列，先明确告知用户当前版本，再决定是否升级或降级到 `openclaw@v2026.4.26`。

## Phase 3: 选择 Qwen 计划

优先推荐标准（按量付费）端点，尤其当用户要使用 `qwen3.6-plus` 时。Coding Plan 支持可能落后于公开模型目录。

| 计划 | 区域 | Auth choice | 端点 |
|---|---|---|---|
| 标准（按量付费） | China | `qwen-standard-api-key-cn` | `https://dashscope.aliyuncs.com/compatible-mode/v1` |
| 标准（按量付费） | Global | `qwen-standard-api-key` | `https://dashscope-intl.aliyuncs.com/compatible-mode/v1` |
| Coding Plan（订阅） | China | `qwen-api-key-cn` | `https://coding.dashscope.aliyuncs.com/v1` |
| Coding Plan（订阅） | Global | `qwen-api-key` | `https://coding-intl.dashscope.aliyuncs.com/v1` |

默认推荐：

- 计划：`standard`
- 区域：`china`
- 默认模型：`qwen/qwen3.5-plus`

常用模型：

| 模型引用 | 备注 |
|---|---|
| `qwen/qwen3.5-plus` | 默认模型，文本/图像，长上下文 |
| `qwen/qwen3.6-plus` | 需要时优先使用标准端点 |
| `qwen/qwen3-max-2026-01-23` | Qwen Max 系列 |
| `qwen/qwen3-coder-plus` | 编码模型 |
| `qwen/qwen3-coder-next` | 编码模型 |

## Phase 4: 配置 Qwen API Key

从 `home.qwencloud.com/api-keys` 创建或复制 API key。

首选：

```bash
export QWEN_API_KEY="sk-..."
```

Agent 执行安装时不要依赖 `openclaw onboard`，因为它会进入交互式界面。默认使用 Phase 6 的配置脚本直接写入 `models.providers.qwen`、endpoint、API key 和 `agents.defaults.model.primary`，从而无交互完成配置。

`openclaw onboard --auth-choice ...` 只作为人工手动配置的备选方式，不是本 Skill 的默认路径。

## Phase 5: 安装钉钉 Connector

OpenClaw 4.26 推荐使用 DingTalk Connector：

```bash
openclaw plugins install @dingtalk-real-ai/dingtalk-connector
```

国内网络慢时：

```bash
NPM_CONFIG_REGISTRY=https://registry.npmmirror.com openclaw plugins install @dingtalk-real-ai/dingtalk-connector
```

验证：

```bash
openclaw plugins list
```

期望看到 DingTalk Connector 已安装/加载。

## Phase 6: 写入 OpenClaw 配置

配置脚本会 deep merge `~/.openclaw/openclaw.json`，不会清空已有 `meta`、`plugins.installs`、`gateway.auth` 等系统字段。

标准 China + 默认模型：

```bash
python3 /path/to/install-openclaw/scripts/configure_openclaw_dingtalk.py \
  --plan standard \
  --region china \
  --qwen-api-key "$QWEN_API_KEY" \
  --model-id qwen3.5-plus
```

标准 China + 钉钉 Connector：

```bash
python3 /path/to/install-openclaw/scripts/configure_openclaw_dingtalk.py \
  --plan standard \
  --region china \
  --qwen-api-key "$QWEN_API_KEY" \
  --model-id qwen3.5-plus \
  --dingtalk-client-id "dingxxxxxx" \
  --dingtalk-client-secret "your-secret"
```

Coding Plan China + 钉钉 Connector：

```bash
python3 /path/to/install-openclaw/scripts/configure_openclaw_dingtalk.py \
  --plan coding \
  --region china \
  --qwen-api-key "$QWEN_API_KEY" \
  --model-id qwen3-coder-plus \
  --dingtalk-client-id "dingxxxxxx" \
  --dingtalk-client-secret "your-secret"
```

兼容旧环境变量：

```bash
python3 /path/to/install-openclaw/scripts/configure_openclaw_dingtalk.py \
  --dashscope-api-key "$DASHSCOPE_API_KEY" \
  --dingtalk-client-id "dingxxxxxx" \
  --dingtalk-client-secret "your-secret"
```

如果用户明确希望人工进入 OpenClaw onboard/auth store，而不想在 `openclaw.json` 写入 `models.providers.qwen`，才加：

```bash
--no-write-provider-config
```

这种方式会回到交互式 `openclaw onboard --auth-choice ...` 流程，不适合 Agent 无人值守安装。

脚本写入的关键配置：

- `agents.defaults.model.primary = qwen/<model-id>`
- `models.providers.qwen`，默认写入，按 plan/region 选择 endpoint
- 传入钉钉凭证时写入 `channels.dingtalk-connector`
- 传入钉钉凭证时写入 `plugins.allow` / `plugins.entries.dingtalk-connector`
- `gateway.mode = local`
- `skills.load.extraDirs = /usr/share/anolisa/skills`
- `qwen.plan` / `qwen.region` / `qwen.authChoice` 作为可读元数据

## Phase 7: 启动 Gateway

初始化/修复

```bash
openclaw doctor --fix
```

启动：

```bash
openclaw gateway --force
```

验证：

```bash
openclaw health
openclaw gateway status
openclaw channels status --probe
```

告诉用户可以进行实际的模型验证：

```bash
openclaw agent --message "hello, introduce yourself briefly" --agent main
```

## Phase 8: 钉钉开发者平台

1. 访问 [钉钉开发者后台](https://open-dev.dingtalk.com/) 创建企业内部应用。
2. 添加机器人能力。
3. 消息接收模式选择 Stream 模式。
4. 复制 AppKey 作为 `--dingtalk-client-id`。
5. 复制 AppSecret 作为 `--dingtalk-client-secret`。
6. 发布应用。

钉钉 Connector 常用配置项：

| 字段 | 默认 | 说明 |
|---|---|---|
| `enabled` | `true` | 启用频道 |
| `clientId` | 必填 | AppKey |
| `clientSecret` | 必填 | AppSecret |
| `sharedMemoryAcrossConversations` | `true` | 跨会话共享长期记忆 |
| `separateSessionByConversation` | `true` | 按会话隔离 session |
| `groupSessionScope` | `group` | 群会话共享，或 `group_sender` 按发送者隔离 |

不要写入未被插件 schema 支持的字段，例如 `gatewayToken`。多余字段会导致 `must NOT have additional properties`。

## 服务管理速查

| 操作 | 命令 |
|---|---|
| 启动/重启 gateway | `openclaw gateway --force` |
| 健康检查 | `openclaw health` |
| gateway 状态 | `openclaw gateway status` |
| 模型状态 | `openclaw models status` |
| Qwen 模型列表 | `openclaw models list --provider qwen` |
| 插件列表 | `openclaw plugins list` |
| 频道探测 | `openclaw channels status --probe` |
| 前台调试 | `openclaw gateway run --verbose` |
| 日志 | `openclaw logs --follow` |

## 排障优先级

1. `openclaw --version` 是否为 4.26 系列。
2. `openclaw models list --provider qwen` 是否能列出 Qwen 模型。
3. `agents.defaults.model.primary` 是否使用 `qwen/...`，不是旧 `bailian/...` 或 `modelstudio/...`。
4. 如果使用 `qwen3.6-plus`，是否选择标准端点而非 Coding Plan。
5. `QWEN_API_KEY` 是否可被 gateway 进程读取。
6. DingTalk Connector 是否安装并加载。
7. `channels.dingtalk-connector` 是否只包含 schema 支持字段。

常见问题详见 `references/troubleshooting.md`。
