# 通用工具 Provider 协议

[English](provider-protocol.md)

本 fork 为四个原生适配器实现了实验性的 `aw-provider/v1alpha1`。Provider 使用同一套
请求和响应格式，AW 归一化原生输入、校验效果并转换返回值，回调调度仍由宿主负责。
已有 `native-hook/v1alpha1` 继续透传原生输入、输出和退出码。本切片尚未接入待合入的
Core/Journal。

## 传输与准入

AW 为每个方法启动一次配置中的 literal `transport.argv`。子进程从 stdin 读取一份
UTF-8 JSON 对象和 EOF，在 stdout 返回一份 JSON 对象；诊断写 stderr。策略阻断也
必须退出 0，非零表示协议执行失败。Provider 可以是脚本、二进制或后台服务的 CLI
客户端；其依赖单独安装，AW 不推断或代装。

`aw validate` 只校验配置语法和引用；`aw plan` 增加适配器与效果准入，不执行 Provider。
`aw check AGENT` 对启用的结构化步骤执行 `describe` 和 `validate_config`，整体准入
上限 30 秒。`aw run` 在生成接线、启动 Agent 前执行相同检查。准入通过不能证明插件
已安装或效果已被宿主采用。

每次结构化回调在 `invoke` 前重新执行发现与配置校验，避免使用过期的发现缓存。
三个子进程共享该 Provider 的 `timeout_ms`；下一次调用使用扣除前序执行和回收时间
后的预算。回收可使用 runner 的额外有界宽限。`max_output_bytes` 分别约束每个
子进程的输出流，不自动重试。这一简单实现每次回调启动三个进程；持久连接和发现缓存
留待后续。

原生调度模式没有 AW 整事件截止时间。`default_event_budget_ms` 仍为保留字段；
显式 event `budget_ms`、guard、非通配工具 selector、ask 和变换效果在运行准入时拒绝。
结构化步骤当前支持工具前 `observe/block`、工具后 `observe`；`on_error` 工具前支持
`block/report`，工具后支持 `report`。Provider 描述必须包含配置要求的 operation、
event 和 effects，但声明不能扩大适配器实际准入能力。

## 消息格式

所有请求都有 `api_version`、`method` 和新生成的 `request_id`。成功响应返回相同
版本和 ID，以及 `status: "ok"`。响应未知字段、重复类型字段、多份 JSON、身份不匹配
均失败；不能通过可选字段夹带原生控制效果。

```json
{"api_version":"aw-provider/v1alpha1","method":"describe","request_id":"example-1"}
```

```json
{"api_version":"aw-provider/v1alpha1","request_id":"example-1","status":"ok","operations":[{"name":"check","events":["tool.before"],"effects":["observe","block"]},{"name":"record","events":["tool.after"],"effects":["observe"]}]}
```

operation 名非空且唯一，最多 64 项。Provider 定义并校验自己的私有 `config`，拒绝
未知或非法字段；AW 不重新解释该对象，也不将其作为 Agent 的原生设置。

```json
{"api_version":"aw-provider/v1alpha1","method":"validate_config","request_id":"example-2","config":{"blocked_substrings":["AW_DENY_FIXTURE"]}}
```

```json
{"api_version":"aw-provider/v1alpha1","request_id":"example-2","status":"ok"}
```

调用携带 operation、私有配置、精确配置 revision、剩余预算、允许效果、归一化事件
和不透明摘要：

```json
{
  "api_version": "aw-provider/v1alpha1",
  "method": "invoke",
  "request_id": "example-3",
  "operation": "check",
  "config_revision": "configuration-file-sha256",
  "budget_ms": 4500,
  "allowed_effects": ["observe", "block"],
  "input_digest": "sha256:opaque-request-binding",
  "config": {"blocked_substrings": ["AW_DENY_FIXTURE"]},
  "event": {
    "name": "tool.before",
    "agent": {"adapter": "qoder", "binding_id": "qoder", "instance_id": null},
    "session_id": "native-session-id",
    "tool": {
      "name": "Bash",
      "native_name": "Bash",
      "call_id": "native-call-id",
      "input": {"command": "printf AW_DENY_FIXTURE"},
      "result": null
    },
    "native": {"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": "printf AW_DENY_FIXTURE"}}
  }
}
```

`binding_id` 是配置目标名，不是运行会话身份。缺失的原生 ID 为 null，不捏造实例身份、
工具语义分类或来源保证。任意工具名与 JSON 对象参数保留。工具后结果保持原始 JSON
类型，包括 Hermes 的序列化字符串和结构化、多模态对象。错误与状态信息保留在
`native`；result 为 null 不代表成功。QwenPaw 的参数 JSON 字符串解析为 `tool.input`，
原始字符串继续保留在 `native`。

`config_revision` 对配置文件原始字节求摘要。`input_digest` 对 AW 加入摘要字段前的
紧凑序列化调用求摘要，包含请求 ID、配置、预算和事件。Provider 原样回传，无需复现
序列化算法。这是请求绑定标识，不是 `aw-contracts` 的 canonical wire 编码或经过认证的
采用凭据；私有 Unicode 键和 JSON 小数不受该 wire 格式约束。

```json
{"api_version":"aw-provider/v1alpha1","request_id":"example-3","status":"ok","input_digest":"sha256:opaque-request-binding","effects":[{"type":"block","reason_code":"policy_match"}]}
```

effects 最多 64 项，每项包含 `type` 和可选 `reason_code`，只能返回配置已允许的效果。
原因码如有，长度为 1–128，仅允许 ASCII 字母、数字、`_`、`-` 和 `.`。`effects: []`
表示不增加限制，`{"type":"observe"}` 表示观察；两者都不代替宿主批准权限。任一合法
block 转换为宿主拒绝响应。Provider stderr 和原因码不复制到宿主输出或审计，宿主
接收固定 AW 诊断。

错误响应使用公共头部、`status: "error"` 和 `error_code`，不包含 effects。协议错误、
超时、输出超限、非零退出、私有配置非法、请求 ID 或摘要不匹配，都按照 `on_error`
处理，并记录为失败，不记为成功的策略命中。

## 原生映射与故障边界

| 适配器 | 中性响应 | 阻断响应 |
| --- | --- | --- |
| Qoder | exit 0，`{}` | exit 2，stderr 固定原因 |
| OpenClaw | exit 0，`{}` | exit 0，`block: true` 和 `blockReason` |
| Hermes | exit 0，`{}` | exit 0，`action: block` 和 `message` |
| QwenPaw | exit 0，`{}` | exit 2，适配器产生 DENIED ToolResponse |

生成的结构化接线把 adapter 与故障策略传给 `aw hook --adapter ADAPTER --on-error
block|report`，因此仍存活的客户端也能转换 daemon 断连和输入错误。若只返回通用
exit 125，Qoder 和部分 Hermes shell-Hook 路径会放行。原生模式保持既有退出码语义。

此机制不保证回调之外的强制执行。Qoder helper 缺失、被杀或由宿主判超时仍可能放行；
Hermes 仍需用户同意 shell Hook，启动时拒绝会关闭注册的 `HERMES_SAFE_MODE` (`1/true/yes/on`)；
OpenClaw 必须实际加载插件；QwenPaw 外部执行工具不经过 `on_acting`。其 App 入口
加载外部插件，ACP/TUI 则不加载并已拒绝。App 验收等待原生插件加载状态，端口监听
不代表 Hook 就绪；被拒绝工具也可能按原生 middleware 嵌套产生 after 观察。后续原生 Hook
改参数也可能让先前检查失效。`required: true` 不会将这些路径升级成 final/protected。

审计在原有调用元数据上增加 `protocol` 和 `disposition`（`native/observe/block/error`）。
AW 不记录事件正文、Provider 输出、环境或 argv。审计中的 block 表示 AW 返回了阻断，
是否实际采用仍需框架证据。

## 样例与 sec-core 边界

[aw.provider.yaml](../../crates/aw-cli/examples/aw.provider.yaml) 给四个目标复用同一份
[示例 Provider](../../examples/providers/policy.py)。子串规则是接线验收样例，不是安全
策略引擎。可选 `sec_core` 对象提供绝对路径 CLI argv 前缀、`timeout_ms`、
`block_verdicts`（默认 `warn/deny`）。样例仅识别已确认的框架与工具名组合，取出 shell
命令后以 literal argv 调用 `scan-code --code COMMAND --language bash --mode regex`，
不经 shell 拼接。

sec-core 的 pass、warn、deny 都可能退出 0，封装必须读取 JSON verdict；扫描错误作为
Provider 错误处理。其他工具不会被推断成 shell。已有 Python V1 本地执行扫描，Rust
V2 CLI 依赖独立管理的 sec-core daemon。本适配不替换该服务，也未实现规划中的自定义
Hook 策略管理；完整规则、工具覆盖和生产安全语义仍是与 sec-core 的联合交付内容。

本地联合验收使用已安装的 sec-core V1 0.8.0 和真实 Qoder：pass 允许无害 printf
并产生 after；带引用规则触发文本的第二次 printf 返回 warn，被阻断且没有 after。
此证据验证真实扫描器与宿主响应链路，不认证仓库 V2 版本或完整安全覆盖。
