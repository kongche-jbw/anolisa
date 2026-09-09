# 通过原生 hook 调用 Tokenless

[English](tokenless-projection.md)

`aw-tokenless-host` 把现有 AW projection 合同连接到 Tokenless 0.8.0 的
原生 Protocol v2，直接复用仓库中的 `tokenless-protocol` crate 校验请求和
响应。AW 不实现压缩算法，也不另造一套 Tokenless 协议。

## 调用与证据边界

Qoder 的可选 hook 使用一份固定顺序的 plan：

1. 由现有 Adapter 捕获成功工具结果中的文本。
2. 现有 SecCore 对原文执行内容检查。
3. Core 允许继续时，通过有时限的进程 Host 执行 Tokenless `compress`。
   两次调用使用同一份输入摘要、scope 和 plan。
4. 校验原生响应的身份、操作和候选，将真实 Receipt 与 Core 最终记录写入
   现有 FileJournal。
5. 只有 Core 允许继续，且候选 UTF-8 字节数确实减少时，才准备 Qoder 的
   `hookSpecificOutput.updatedToolOutput`。
6. 先写 hook 证据，再返回替换内容。原生历史中的采用情况由独立操作验证。

Receipt 的 `produced` 只表示生成了候选。`delivery=prepared_for_return`
只表示 hook 准备返回候选，不代表 Qoder 已经采用。hook 写出的证据始终从
`adoption=not_observed` 开始。独立历史验证器可以确认 `local_history`，
但不能据此宣称候选进入最终模型请求。Qoder PostToolUse profile revision 2
声明了这项观察能力，`result_finality` 仍为 `subject_to_later_change`。
Codex 当前 hook 只能观察结果，配置 Tokenless 时会在调用前明确拒绝。

SecCore 仍属于观察模式；返回压缩候选时，检查发现的提示也会保留。必需的
检查失败后，Tokenless 不会执行。这份 hook 不替换原有安全策略或 OS 防护。

## 恢复保证为何降低

Tokenless 原生 `lossless` 指没有删除与任务相关的信息。例如，JSON cleanup
可能移除空字段。AW 的 `lossless` 要求独立恢复逐字节相同的原文，而当前
Adapter 没有这样的 decoder。

因此，hook 明确接受 AW `unrecoverable`，把原生 `lossless` 和 `unrecoverable`
候选统一映射为 AW `unrecoverable`、`recovery:none`。不含原文的
`tokenless_mapping` 记录原生声明、降低后的 AW 保证、原因和原生响应摘要，
并由 Receipt evidence 绑定该记录的摘要。调用方如果只接受 `lossless` 或
`retrievable`，Host 会拒绝；原生 retrievable 结果及 stash 引用也会拒绝。
这里没有虚构 resolver 或恢复工具。

这是一项明确允许信息损失的策略，不能宣传为逐字节无损。后续若实现独立
可验证的恢复能力，应更新固定的 profile revision。本次 Schema 文件未改动。

## 启动配置

保留现有 SecCore 的 `provider` 配置，新增 `tokenless` 对象，字段同样为
`provider_id`、`provider_version`、绝对路径 `program`、`args` 和
`environment`，并必须显式设置 `allow_unrecoverable:true`。缺失或设为 false
都会在 Provider 调用前拒绝；仅启用 Provider 不代表同意信息损失。使用从源码构建的 Tokenless 0.8.0，参数为
`args:["compress"]`，并指定隔离目录的绝对路径 `TOKENLESS_DATA_DIR`。
本机安装的 0.7.0 没有 `compress`；旧 PoC 的原生 Protocol v1 也不兼容。
Host 不会自动回退到这两种版本。

Host 固定可执行文件字节及完整的有效启动配置，强制
`TOKENLESS_COMPRESSION_ENABLED=1`、`TOKENLESS_STATS_ENABLED=0`、
`TOKENLESS_SLS_ENABLED=0`，避免另起一份原生压缩统计。所选 Agent 配置中
还必须停用原生 Tokenless 插件；Host 无法发现或关闭无关的原生 hook。
受控 Qoder 验收只加载专用的 project settings。

Qoder scope 仍需要启动器提供有界单轮的标识，因此这还不是交互多轮启动器。
hook 不复制原文或认证材料；候选正文保存在私有证据中，供后续匹配原生历史。
`calls[].invocation` 只省略 `input.artifact.content`，历史验证器从捕获的
原生事件中重建这一字段，再核对现有输入摘要。

## 失败行为与验证

无节省、dry run、原生 passthrough 会产生 bypass Receipt，不返回替换。
原生错误、超时、身份不符、可执行文件变化、协议错误、恢复能力不支持都
保留可见的失败。这些结果不累计候选或采用节省。Host 对请求、响应、stderr
和运行时间设限，只终止自己创建的进程组。进程传输沿用现有 SecCore Host
实现；为避免本次改变 SecCore，将传输抽成共享 crate 的工作留待后续。

在 `src/aw` 执行：

```bash
cargo test -p aw-tokenless-host -p aw-adapters -p aw-hook-cli --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo doc --workspace --no-deps --locked
```

Rust 集成测试使用合成原生进程，执行的是实际 Adapter/Core/Host/Journal
链路，不能当作真实 Agent 验收。在 Linux ARM64 上，对源码构建的
Tokenless 0.8.0 做有界直接测试，80 条合成 JSON 记录经 `json_cleanup`
从 12,134 字节缩减到 6,134 字节。真实 Agent、历史及 Herdr 验收另行记录
确切二进制版本和会话范围。
