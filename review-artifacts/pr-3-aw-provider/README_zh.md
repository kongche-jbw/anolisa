# PR 3 AW Provider 审查材料

[English](README.md)

本目录说明
[casparant/anolisa PR 3](https://github.com/casparant/anolisa/pull/3)
提出的架构，以及
[`kongche-jbw/anolisa:feat/aw/provider-e2e-poc`](https://github.com/kongche-jbw/anolisa/tree/feat/aw/provider-e2e-poc)
实现的修正基线。原 PR 审查基线固定为
`8574ecb022ec9ffc68e1a71e30f2186b6ec81674`，头提交固定为
`42d07649409ecd5bb023056b28545efbd9325ef2`。修正 PoC 及最终 VM 证据固定为
`5ebfc0b3905fa2f5f74aff2da4aec2b3be639647`。

PR 3 的大方向成立。Canonical Capability Contract、AW Core、通用 Provider Host、组件
Provider、Environment 最终权力与 content-free Ledger 的分层值得保留。原 PR 头提交仍有
跨边界语义没有闭合，fork PoC 用可执行代码验证修复方向。PoC 是后续实现基线，不代表
生产交付已经完成。

## 三种状态

| 标记 | 含义 |
| --- | --- |
| 原 PR | 固定头提交 `42d07649` 上已经存在的行为 |
| PoC 基线 | fork 分支已经实现或用真实组件验证的行为 |
| 仍待完成 | 尚未冻结的合同、运行接线或产品化工作 |

PoC 已明确结果回流。Provider Host 把瞬时 candidate 与不含正文的 receipt 返回 AW Core，
Core 校验后交给 COSH。COSH 决定写入哪一段本地模型历史，随后 AW Ledger 保存类型封闭的
`context_adoption` 事实。它引用 Core 结果形成的完整 `post_tool_use_plan`。候选为空或不满足
严格 `lossless` 时，COSH 保留 source bytes。

仍有两条边界需要继续开发。PreTool 安全检查尚未绑定 COSH 最终执行的准确字节。
Checkpoint 当前采用 Gateway State Provider 与 Guarded Checkpoint V2，它不属于 AW
manifest Provider。Btrfs subvolume 上的 Ubuntu VM + Herdr 正常链路已经通过，覆盖
approval、permit、execution、Guarded V2、durable evidence 与 `task_succeeded`。响应丢失
和进程重启后的 evidence-only 恢复还没有进行故障注入实机验证。

## 建议阅读顺序

| 读者 | 文档 | 阅读目标 |
| --- | --- | --- |
| 管理层与架构负责人 | [AW Provider 架构说明](executive-brief_zh.md) | 理解 Provider 如何生效及剩余决策 |
| PR Reviewer | [完整架构审查](architecture-review_zh.md) | 区分原 PR 问题与 PoC 修复基线 |
| Schema 评审人员 | [Schema 参考与语义讨论](schema-reference_zh.md) | 逐份查看字段、语义和争议点 |
| 组件研发 | [组件接入手册](component-integration_zh.md) | 开发 native endpoint、manifest 与测试 |
| 运行链路研发 | [真实调用实例](runtime-call-examples_zh.md) | 查看 Tokenless、安全与 Checkpoint 字段 |
| Agent Host PoC 负责人 | [PoC 对照](poc-comparison_zh.md) | 确认能力面与主机面的连接位置 |
| 准备提交 Review 的维护者 | [PR Review 评论草稿](pr-review-comment_zh.md) | 提交聚焦逻辑和 Schema 的评论 |

## 交互图

- [Provider 生效架构](provider-effect-architecture.html)
- [Tokenless 生效时序](provider-effect-sequence.html)
- [Agent Sec 命令检查时序](security-command-call.html)
- [Checkpoint State Provider 时序](checkpoint-create-call.html)

三份调用图使用明亮的 Drafter 工程蓝图风格，直接展示简化后的真实字段值。总体架构图是
自包含 Archify 页面。十四张浅色 Schema 图位于 `images/schemas/`。

构建打包与 CI 接线不属于本轮审查重点。生产交付前仍必须完成这些工作。
