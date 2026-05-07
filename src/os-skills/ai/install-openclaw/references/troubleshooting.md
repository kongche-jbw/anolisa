# OpenClaw 4.26 排查手册

本文件记录 OpenClaw 4.26 + Qwen provider + DingTalk Connector 的常见问题。

## 1. 版本不是 4.26

检查：

```bash
openclaw --version
which openclaw
```

修复：

```bash
npm install -g openclaw@4.26
```

国内网络慢时：

```bash
npm install -g openclaw@4.26 --registry=https://registry.npmmirror.com
```

## 2. 仍在使用旧 provider 引用

现象：

- `agents.defaults.model.primary` 是 `bailian/...` 或 `modelstudio/...`
- `openclaw models list --provider qwen` 没有被使用

OpenClaw 4.26 推荐：

```json
{
  "agents": {
    "defaults": {
      "model": {
        "primary": "qwen/qwen3.5-plus"
      }
    }
  }
}
```

修复：

```bash
openclaw models set qwen/qwen3.5-plus
```

## 3. Qwen API key 找不到

现象：

- `No API key found for provider "qwen"`
- gateway 进程启动了，但模型调用 401

优先使用：

```bash
export QWEN_API_KEY="sk-..."
```

Agent 无交互安装时，优先用配置脚本直接写入 `models.providers.qwen`：

```bash
python3 scripts/configure_openclaw_dingtalk.py \
  --qwen-api-key "$QWEN_API_KEY" \
  --plan standard \
  --region china
```

人工手动配置时才运行对应 onboard：

```bash
openclaw onboard --auth-choice qwen-standard-api-key-cn
```

兼容变量：

- `MODELSTUDIO_API_KEY`
- `DASHSCOPE_API_KEY`

如果 shell 中有 key 但 gateway 仍找不到，说明 gateway 进程没有继承环境变量。Agent 安装时应使用配置脚本写入 provider `apiKey`；人工安装时可写入 OpenClaw onboard/auth store，或写入 systemd 用户服务环境。

## 4. 端点和计划不匹配

OpenClaw 4.26 Qwen 端点：

| 计划 | 区域 | Auth choice | 端点 |
|---|---|---|---|
| 标准 | China | `qwen-standard-api-key-cn` | `https://dashscope.aliyuncs.com/compatible-mode/v1` |
| 标准 | Global | `qwen-standard-api-key` | `https://dashscope-intl.aliyuncs.com/compatible-mode/v1` |
| Coding Plan | China | `qwen-api-key-cn` | `https://coding.dashscope.aliyuncs.com/v1` |
| Coding Plan | Global | `qwen-api-key` | `https://coding-intl.dashscope.aliyuncs.com/v1` |

如果使用 `qwen3.6-plus`，优先选择标准端点；Coding Plan 的模型支持可能落后于公开目录。

## 5. gateway.mode 缺失

现象：

```text
Gateway start blocked: set gateway.mode=local
```

修复 `~/.openclaw/openclaw.json`：

```json
{
  "gateway": {
    "mode": "local"
  }
}
```

配置脚本 `scripts/configure_openclaw_dingtalk.py` 会自动写入该字段。

## 6. DingTalk Connector 未加载

检查：

```bash
openclaw plugins list
openclaw channels status --probe
```

安装：

```bash
openclaw plugins install @dingtalk-real-ai/dingtalk-connector
```

国内网络慢时：

```bash
NPM_CONFIG_REGISTRY=https://registry.npmmirror.com openclaw plugins install @dingtalk-real-ai/dingtalk-connector
```

确认 `~/.openclaw/openclaw.json`：

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
  }
}
```

## 7. DingTalk 配置校验失败

现象：

```text
must NOT have additional properties
```

原因：`channels.dingtalk-connector` 中写入了插件 schema 不支持的字段，例如 `gatewayToken`。

保留最小配置：

```json
{
  "channels": {
    "dingtalk-connector": {
      "enabled": true,
      "clientId": "dingxxxxxx",
      "clientSecret": "your-secret",
      "sharedMemoryAcrossConversations": true,
      "separateSessionByConversation": true,
      "groupSessionScope": "group"
    }
  }
}
```

## 8. DingTalk Stream 连接 400

常见原因：

| 原因 | 修复 |
|---|---|
| 应用未发布 | 钉钉开发者后台发布应用 |
| 凭证错误 | 检查 AppKey / AppSecret |
| 非 Stream 模式 | 机器人消息接收模式改为 Stream |

## 9. 直接写配置慢或不可靠

避免用大量 `openclaw config set` 逐项写配置。配置脚本使用 JSON deep merge，直接写 `~/.openclaw/openclaw.json`，会保留系统字段并覆盖目标字段。

## 快速诊断

```bash
openclaw --version
openclaw models list --provider qwen
openclaw models status
openclaw plugins list
openclaw channels status --probe
openclaw health
openclaw logs --follow
```

检查关键配置：

```bash
python3 - <<'PY'
import json, os
p = os.path.expanduser('~/.openclaw/openclaw.json')
d = json.load(open(p))
print('model.primary:', d.get('agents', {}).get('defaults', {}).get('model', {}).get('primary'))
print('gateway.mode:', d.get('gateway', {}).get('mode'))
print('plugins.allow:', d.get('plugins', {}).get('allow'))
print('dingtalk:', d.get('channels', {}).get('dingtalk-connector', {}))
print('qwen:', d.get('qwen', {}))
PY
```
