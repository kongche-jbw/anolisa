# Cosh-ng RuntimeAdapter 设计

[English](cosh-ng-runtime-adapter.md)

Cosh-ng RuntimeAdapter 把 Cosh Hook 事件翻译为 Agent Memory Protocol v1
请求。它是可替换的宿主适配层，不属于 Memory backend。因此同一
backend 可以同时服务 Cosh-ng、DeepSeek Harness、MCP 或其他 Runtime，
无需在存储层固化 Runtime 的 Hook 名称。

## 事件映射

| Cosh 事件 | 协议行为 | 用户任务行为 |
|---|---|---|
| `SessionStart` | 打开 session，再以 `session_resume` 召回 | 有结果时注入有界的恢复上下文 |
| `UserPromptSubmit` | 幂等懒打开 session，再以 `turn` 召回 | 针对原始逻辑 prompt 注入有界上下文 |
| `PostToolUse` | 追加脱敏的 `tool_completed` 证据事件 | capture 失败时仍继续用户任务 |
| `PostToolUseFailure` | 追加脱敏的 `tool_failed` 证据事件 | capture 失败时仍继续用户任务 |
| `AfterModel` | 不捕获最终结果 | 无影响 |
| `Stop` | 不捕获最终结果 | 无影响 |

Shell 托管的 transport 可以抑制 `SessionStart`。因此
`UserPromptSubmit` 在召回前也会执行同样的幂等 session 激活。查询
使用 Hook 中的原始逻辑 prompt，不使用 provider 封装或生成的
system context。

`AfterModel` 和 `Stop` 都早于 Cosh 最终提交 assistant turn。Stop
Hook 还可以拦截候选答案并触发新的模型循环。所以它们不得生成
Fact、TaskState 或 `turn_committed` 事件。未来可以增加只读的提交后
Runtime 事件和持久化 outbox，这不需要改变 Memory Protocol。

## 信任与降级边界

`IdentityContext` 由可信宿主构造。Hook payload 和模型文本不能改写
tenant、team、user、Agent 或 workspace 授权。本地进程以 OS user 为可信
principal，canonical workspace 指纹仅是本地 scope key。B 端部署必须
绑定网关身份，不能把 `cwd` 当成授权边界。

身份或 scope 无法建立时，Memory 访问 fail-closed。Recall 或 capture
不可用、超时、格式错误或版本不兼容时，用户任务 fail-open。适配器返回
allow 且不注入 Memory 上下文，也不会把错误文本写入模型 prompt。

召回内容即使具有 verified 或 normative 的事实权威，仍是不可信数据。
适配器使用固定 wrapper、转义内容、保留来源，并再次执行 item、byte
和 token 预算。只有真正进入 `additional_context` 的 item 才会上报为
admitted，其他项上报 dropped，usefulness 保持 `unknown`。仅被检索到不等于
有效命中。

## 工具证据

工具 capture 仅保存有界的脱敏摘要、结果分类、工具名称以及可用的
不透明引用或摘要，默认不保存无界命令输出。Idempotency key 由稳定的
session、run、tool-use 和 Hook event 身份派生，因此 Cosh 重试在符合
协议的 backend 上只产生一次效果。

成功和失败的工具调用都只是不可变证据。它们不会自动升级成已验证
建议、策略或可恢复 TaskState。

## 打包与一致性

`adapters/cosh-ng/cosh-extension.json` 为四种受支持事件分别注册一个 fail-open
命令 Hook，并且有意不注册 `AfterModel` 和 `Stop`。
`agent-memory-cosh-hook` 从标准输入读取一个有界 Cosh Hook object，并向
标准输出写入一个 Hook response object。

阶段 2 的可执行文件使用 process-local conformance backend 验证适配边界。
跨进程持久化和用户可感知的恢复由 typed local backend 在下一阶段提供。
替换 backend 不会改变 Hook 映射或 Cosh extension manifest。阶段 2 的开发资产
不会被 Makefile 或 RPM 安装。只有在阶段 3 的可执行文件接入持久化存储后
才开放打包，避免把 process-local ack 宣传成可跨 Hook 恢复的 Memory 功能。

## 验收不变量

- 每个外部用户 prompt 最多触发一次 turn recall。
- `SessionStart` 被抑制时，仍会有一次幂等懒激活。
- 空召回不注入 wrapper。
- 非法身份不回退到 shared 数据。
- Recall 和 capture 失败不阻断用户任务。
- Context admission 不改变 Cosh 静态 system 和 tool prefix。
- 成功和失败工具调用使用不同 event outcome。
- 重复工具事件安全重放同一 mutation。
- `AfterModel`、`Stop`、cancel 和 provider failure 都不会生成已提交最终结果。
- Admission report 与真正注入的 item ID 一致。
