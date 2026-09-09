# AW Core 实现基线

[English](core-baseline.md)

Core 0.1.0 实现现有合同校验之间的执行逻辑。21 份已注册 Schema 和 8 份保留的
PoC 评审 Schema 原文不变。这次固定的是供评审的实现版本，不代表 Schema 提案
已获接受，也不另建统一 Agent 消息协议。

## 结构与接入

根目录 `aw-contracts` 保持纯校验库。`crates/aw-core` 负责准备计划、执行及私有
日志存储。接入方构造 `Core`，实现 `ProviderHost` 和 `Clock`，通过
`PrepareRequest` 提供可信计划、boundary、runtime，以及每步的 `StepInput`。
`prepare` 在首次调用前解析全部选中 Provider，并校验全部 invocation。空路由
保留为缺口；明确指定的 Provider 不存在时，整个准备阶段失败。

`execute` 消费不可修改的 `PreparedPlan`，通过 `Journal` 和 `Cancellation`
完成执行。事件占用键绑定完整 scope 与 event ID，调用 ID 绑定计划摘要、步骤
和所选 Provider。同一 scope 下的事件即使换一个计划也不能重复执行，不同 scope
则分别记录。runtime 和 descriptor 必须由接入方认证，JSON 相同不等于来源可信。

Host 负责注册 driver、映射原生协议并限制执行时间和输出大小。Core 不理解组件
命令行，不启动传输服务，也不实现扫描器。每次调用前会重新核对 descriptor，
保持原始绝对 deadline；返回后检查 Receipt，并拒绝超出本地观测时限的成功结果。
Core 无法强制中断阻塞的 Host，硬性时间和内存限制仍由 Host 实现。取消在调用之间
检查，不中断正在执行的 Provider。接入方还需保证执行窗口内的运行时归属有效，
处理退出和重启的竞争。

Core 接收策略已确定的计划，不额外规定发现优先级或框架 hook 顺序。步骤串行
执行，同一步的全部选中 Provider 都结算后再归并结果。必需的 command 检查一旦
返回 warn/deny，其他 Provider 的 allow 不能将其改成 proceed。必需步骤缺结果时
按计划拒绝派发或保留原结果；只有明确允许忽略的可选缺口才继续。取消、拒绝或
保留原结果后，后续步骤记为 skipped。最终生成的记录交给现有
`validate_plan_execution` 再做完整校验。

## 日志与失败行为

`FileJournal` 使用 Linux 本地可信目录，要求文件系统正确实现同步写入。新目录
和文件使用私有权限。原子创建文件使事件占用跨进程、跨重启保留。Core 先确认
事件占用与调用开始记录已写入，再调用 Provider；随后保存不含正文的 Receipt
和步骤结果，最后确认终态追加成功才返回执行结果。

私有 format-1 记录包含序号、前一条摘要、记录和本条摘要。它是存储格式，不是
新增 AW Schema，也不是通用 Ledger 服务。Core 持久化计划、ID、摘要、Receipt
和执行事实，不写入原始能力输入输出。`Execution::calls` 在内存中保留输入输出，
由接入应用决定如何保护、保留和回读制品。Receipt 里的计量仍是 Provider 报告，
不会自动变成采用统计。

Host 传输错误、返回值不合法、成功结果迟到或存储失败都会返回错误，事件占用
继续保留。派发后日志未结束，表示需要核对，不能据此断言 Provider 没执行。
当前没有自动恢复、重试、删除占用或重新派发接口。

读取时检查规范编码、序号和摘要链，半条记录或断链会拒绝。完整的合法前缀无法
表明尾部是否被整条删除；`read_verified` 可用独立保存的最后确认记录核对。
如果攻击者能同时改写存储和外部确认，摘要不能提供真实性保证。非 Linux 平台的
`FileJournal::new` 明确返回不支持；其他存储可实现 `Journal` 接口。

## 交还原生执行边界

`Execution::record`、`calls` 和 `journal_ack` 提供实际关联的执行结果，不派发
工具，也不写会话。确认采用时，adapter 独立观测实际文本与恢复文本，调用
`Registry::validate_plan_adoption`。工具执行前，由原始拥有者重新捕获 intent
和独立 OS 防护状态，通过 `Registry::validate_dispatch`，并保证校验与原生
动作之间没有未检查的变更。Core 返回 proceed 不等于获得这项许可。

本次未接入 Qoder、Codex、OpenClaw、Hermes、Tokenless 或 SecCore 的真实运行
路径，也不准入 checkpoint/restore 状态效果。

## 复现与验证范围

在仓库检出目录，以普通用户执行：

```bash
cd src/aw
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo doc --workspace --no-deps --locked
python3 tests/check_canonical.py
cargo run -p aw-core --example pinned_plan --locked
```

示例使用进程内参考 Provider 和隔离的本地日志，演示 Core 执行，不是实际压缩
或 Agent 验收。合同、执行和日志测试使用合成输入，覆盖失败、重复事件、
descriptor/Schema 差异、取消及存储损坏。测试环境为 Linux ARM64；干净 VM 安装、
其他架构、断电测试和真实 Provider/Agent 接入尚未验证。

## 上游比较与来源

本轮固定查看 casparant 的
[`provider-poc-v4`](https://github.com/casparant/anolisa/tree/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw)。
Caspar Zhang 的 Core 计划实现（`b299cdfe`、`1556e9d6`、`d8902670`）、Host
（`7113de26`）和 Ledger（`7763e2ea`、`192a844e`）为职责拆分提供参考。
其 typed v1 合同、默认决策和 Ledger Schema 无法替代本分支的 v2 能力载荷与
计划约束。本次没有移植上游源文件或提交，继续复用我们已有的规范编码与校验器。
双方仓库均为 Apache-2.0。原生进程 driver 可以在后续 Host 接入中单独复用，
届时记录准确文件来源并适配当前合同。
