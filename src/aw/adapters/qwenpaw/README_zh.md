# QwenPaw 原生适配器

[English](README.md)

此插件为每个 AW hook 注册一个 AgentScope `on_acting` middleware。验证版本为
官方 `agentscope-ai/QwenPaw` 提交
`3822ec7173d17cf37c8a02f51d3ed5628079e86e`（2.2.2b4），依赖 AgentScope 2.0.8。
这里的项目是 QwenPaw，不是 Qwen Code。

将 `plugin.json` 和 `plugin.py` 复制到
`$QWENPAW_WORKING_DIR/plugins/aw-native/`，或对本目录运行
`qwenpaw plugin install`。设置 `AW_BIN`、`AW_SOCKET`、`AW_AGENT` 和
`AW_NATIVE_CONFIG`；最后一个变量指向以下格式的 JSON：

```json
{"hooks":[{"event":"tool.before","provider":"before-command","priority":100},{"event":"tool.after","provider":"after-command","priority":100}]}
```

原生插件注册表按 priority 升序排序，同优先级保留注册顺序。middleware 由外向内
进入，由内向外返回，因此后置 hook 按嵌套逆序执行。适配器不会串行化独立工具，
也不会修改工具的并发标志。

命令接收 `{"event": ..., "tool_call": ...}`；后置命令额外接收
`tool_response`。原生 Pydantic 对象用 `model_dump(mode="json")` 投影。
`tool_call.input` 保留原生 JSON 字符串形式。hook 位于原生权限检查和参数验证之后。

退出码 0 表示观察并继续，stdout 不替换工具结果。前置 hook 退出码 2 返回原生
denied `ToolResponse`，不进入内层工具；其他失败抛出错误。QwenPaw 没有原生命令
hook 传输协议，所以这些退出码是本适配器的明确约定。
JSON 输出中明确的 `action: ask`、`action: approve` 或 `decision: ask` 会抛出
不支持审批的错误。QwenPaw 原生治理审批在 `on_acting` 外部发生，本适配器不声称
提供审批 UI。后置 hook 不能撤销已执行工具。

`tests/native/qwenpaw` 中的限时测试验证已安装的原生 middleware 链。
`live_agent.py` 使用公开 `QwenPawAgent` runtime、原生插件加载器、真实模型和
shell 工具；范围小于完整 HTTP server 或 TUI 测试。设置
`AW_TEST_MODEL_KEY_FILE` 指向私有文件，测试只将 key 读入内存。

原生实验通过 `aw run` 完成三个真实 `qwen3.7-plus` Token Plan 用例：允许时在
临时工作目录写出 `42`；退出码 2 拒绝时没有创建文件，并返回 denied 响应；
显式 ask 在执行前抛出不支持错误。两个前置 hook 按 A、B 执行，两个后置 hook
按 B、A 执行。启动器将后置 wrapper 注册在前置 wrapper 外层，因此后置 hook
也观察到了 denied 响应。没有验证人工审批 UI。
