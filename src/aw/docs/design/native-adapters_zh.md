# 共享原生适配层

[English](native-adapters.md)

`aw-adapters` 0.1.0 提取原生工具文本，将其绑定到可信运行上下文，再通过 `aw-core` 执行已有能力计划。它是供原生集成调用的库；加载边界描述不会安装或启用插件。本次保留所有公开 Schema 和 Core 实现。

## 六类宿主边界

`crates/aw-adapters/profiles/` 下每份文件包含两个已有的 `boundary-descriptor/v1` 对象。外围的 `format: 1` 是库内部资源格式，不是新的 AW 传输协议。

| 宿主 | 前置 / 后置事件 | 命令位置 | 结果位置 | 可观测身份 |
| --- | --- | --- | --- | --- |
| Qoder | `PreToolUse` / `PostToolUse` | `tool_input.command`，工具 `Bash` | `tool_response` 字符串，或已完成 Bash 的 `tool_response.stdout` | `session_id`、`tool_use_id` |
| Codex | `PreToolUse` / `PostToolUse` | `tool_input.command`，工具 `Bash` | `tool_response` 字符串 | `session_id`、`tool_use_id`、`turn_id` |
| Qwen Code | `PreToolUse` / `PostToolUse` | `tool_input.command`，工具 `run_shell_command` | `tool_response` 字符串 | `session_id`、`tool_use_id` |
| Hermes | `pre_tool_call` / `post_tool_call` | `event.args.command`，工具 `terminal` | `event.result` 字符串 | `context.session_id`、`context.tool_call_id` |
| OpenClaw | `before_tool_call` / `after_tool_call` | `event.params.command`，工具 `exec` | `event.result` 字符串 | event/context 中的 `sessionId`、`toolCallId`、`runId` |
| COSH | `PreToolUse` / `PostToolUse` | `tool_input.command`，工具 `run_shell_command` 或 `shell` | `tool_response.llmContent` 字符串 | `session_id`、`tool_use_id` |

Hermes 和 OpenClaw 使用由集成代码根据回调参数构造的**本地** `{event, context}` 容器，这不代表 SDK 自身发送这种消息。OpenClaw 的绑定约定把原生 `runId` 用作 AW `turn_id`；同一身份若同时出现在 event 和 context 中，两处必须一致。Hermes 没有推断出的轮次映射。Qoder 和 Qwen Code 还接受 JSON 字符串形式的 `tool_input`，与已有原生 hook 路径一致。

字段映射依据 `src/agent-sec-core` 中的原生集成，以及 `src/cosh-ng/crates/cosh-core/src/hook.rs` 中的 COSH 事件构造代码。它们说明表内文本位置与源码的兼容关系，不代表已经覆盖各框架的全部能力。

## 提取、准入与执行

1. 原生集成提供 `NativeContext`，包含经过认证的 runtime binding、scope 和稳定的事件发生 ID。身份与权限不能来自模型参数，也不能因为拿到 PID 就视为拥有控制权。
2. `Adapter::capture` 将可观测的原生身份与上下文逐项比对，核对运行代次和会话，再提取一个 UTF-8 文本位置。摘要覆盖原始字节，保留空结果和空白。解码后的原始 payload 完整保留，包括 AW 合同之外的原生字段。提取检查不代表完整的 scope 准入；完整 scope Schema 与计划约束在 prepare 阶段、任何 Provider 调用之前校验。
3. 策略层提供已解析的计划，以及各步骤的约束、预算和截止时间。`Adapter::prepare` 核对事件、输入和边界绑定，构造已有能力输入，再调用 `Core::prepare`。
4. `Adapter::execute` 通过传入的 Provider Host、Journal、Clock 和 Cancellation 接口调用 `Core::execute`，一起返回 Core 记录和原始数据。原生集成按自身策略、审批及响应约定处理结果。

当前桥接实现 `security.content.inspect/v2` 和 `security.code.inspect/v2`。Provider 发现、安全规则、命令派发、文本投影、恢复和采用均不在实现范围内。Core 返回 `proceed`，也不等于已经向原生宿主发出工具许可。

当前只支持表中指定的文本位置。COSH 保留 `returnDisplay`，只提取 `llmContent`；Qoder 已完成、退出码为零且 stderr 为空的 Bash 对象只提取 stdout，保留其他元数据；中断、图像及失败结果拒绝，包括相互矛盾的 `error` 或 `isError` 标记。其他对象结果、数组、多模态内容块和二进制结果会明确报错。原生失败信号，如 `is_error`、interrupted/denied 状态或支持的 SDK error 字段，也会报错。调用方需要保留原生失败处理方式；本库不会把提取错误转成允许执行。

## 为什么边界描述保持保守

原生 hook 能返回阻断，不足以证明它控制了不可绕过的最终输入检查。因此，六份描述都没有声明 final guard、工具派发拒绝权限或采用证明边界，Ledger 要求为 best-effort。Qoder 后置边界记录已有接口的文本替换能力，但当前检查桥接没有执行替换。其余后置边界在本版实现中只提供观测。

前置事件可以提取；**前置计划执行会被现有合同拒绝**，因为这些描述无法建立必需的最终命令检查。启用该路径需要验证原生执行控制，再单独评审新的描述修订。仅把能力标志改成 true 来通过准入，会夸大边界能力。

Receipt 校验能确认调用与输出的记录一致，不能认证扫描器内部行为。执行日志已经持久化，也不能证明原生采用或 OS 防护生效。本次保留原生插件，原路径可继续独立使用；自动共存、防止重复 hook 和原生结果交付仍需真实宿主集成测试。

## 运行与验证

在仓库根目录执行：

```bash
cd src/aw
python3 scripts/check.py
```

桥接测试使用实际适配层和 Core、**合成 Provider** 及内存 Journal，覆盖六类字段映射、身份错配、输入覆盖精确绑定的内容和代码检查、计划绑定、Provider 失败、重复事件及前置拒绝。测试不启动 Agent，也不调用 SecCore 扫描器；通过测试不代表真实框架已启用或生产 Provider 行为已验证。
