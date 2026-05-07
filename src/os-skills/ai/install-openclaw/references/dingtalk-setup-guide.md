# 钉钉 Connector 配置指南

本文件补充 `SKILL.md` 中的钉钉侧配置步骤，适用于 OpenClaw 4.26 的 `dingtalk-connector`。

## 1. 创建钉钉应用

1. 访问 [钉钉开发者后台](https://open-dev.dingtalk.com/)。
2. 创建企业内部应用。
3. 添加机器人能力。
4. 消息接收模式选择 Stream 模式。
5. 发布应用。

## 2. 获取凭证

| 凭证 | 配置脚本参数 |
|---|---|
| AppKey / Client ID | `--dingtalk-client-id` |
| AppSecret / Client Secret | `--dingtalk-client-secret` |
| Robot Code | `--dingtalk-robot-code` |
| Corp ID | `--dingtalk-corp-id` |
| Agent ID | `--dingtalk-agent-id` |

`dingtalk-connector` 的最小可用配置只需要 `clientId` 和 `clientSecret`。其他字段只有在当前钉钉应用或插件版本明确要求时再填写。

## 3. 安装插件

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
openclaw channels status --probe
```

## 4. 配置项速查

| 字段 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `enabled` | boolean | `true` | 启用频道 |
| `clientId` | string | 必填 | AppKey |
| `clientSecret` | string | 必填 | AppSecret |
| `sharedMemoryAcrossConversations` | boolean | `true` | 跨会话共享长期记忆 |
| `separateSessionByConversation` | boolean | `true` | 按会话隔离 session |
| `groupSessionScope` | string | `group` | 群会话共享；可选 `group_sender` |

不要写入未被插件 schema 支持的字段，例如 `gatewayToken`。多余字段会导致 `must NOT have additional properties`。

## 5. 手动配置示例

如不使用脚本，可手动编辑 `~/.openclaw/openclaw.json`：

```json
{
  "plugins": {
    "enabled": true,
    "allow": ["dingtalk-connector"],
    "entries": {
      "dingtalk-connector": {
        "enabled": true
      }
    }
  },
  "agents": {
    "defaults": {
      "model": {
        "primary": "qwen/qwen3.5-plus"
      }
    }
  },
  "channels": {
    "dingtalk-connector": {
      "enabled": true,
      "clientId": "dingxxxxxx",
      "clientSecret": "your-app-secret",
      "sharedMemoryAcrossConversations": true,
      "separateSessionByConversation": true,
      "groupSessionScope": "group"
    }
  }
}
```

## 6. Qwen API Key

Qwen API key 从 `home.qwencloud.com/api-keys` 获取。

OpenClaw 4.26 Agent 无交互安装时，推荐由配置脚本直接写入 `models.providers.qwen` 和 API key，不依赖交互式 onboard：

```bash
python3 scripts/configure_openclaw_dingtalk.py \
  --qwen-api-key "$QWEN_API_KEY" \
  --plan standard \
  --region china
```

人工手动配置时才使用 onboard：

```bash
export QWEN_API_KEY="sk-..."
openclaw onboard --auth-choice qwen-standard-api-key-cn
```

兼容环境变量 `MODELSTUDIO_API_KEY` 和 `DASHSCOPE_API_KEY` 仍可被使用，但新配置应优先使用 `QWEN_API_KEY`。
