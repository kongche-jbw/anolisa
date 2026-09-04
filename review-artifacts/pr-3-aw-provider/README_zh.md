# PR 3 AW Provider 审查材料

[English](README.md)

本目录保存
[casparant/anolisa PR 3](https://github.com/casparant/anolisa/pull/3)
的只读架构审查材料。审查基线固定为
`8574ecb022ec9ffc68e1a71e30f2186b6ec81674`，头提交固定为
`42d07649409ecd5bb023056b28545efbd9325ef2`。

这些文件属于审查证据，不是正式产品文档，也不表示 AW Provider 已经达到生产可用状态。

## 建议阅读顺序

| 读者 | 首选文档 | 目的 |
| --- | --- | --- |
| 管理层与架构负责人 | [面向管理层的架构说明](executive-brief_zh.md) | 理解方向、边界与决策项 |
| 第一次接触 Provider 的研发 | [原理与组件接入手册](component-integration_zh.md) | 从术语到 Tokenless 实际链路 |
| 安全与 Checkpoint 研发 | [运行实例与边界说明](runtime-call-examples_zh.md) | 查看真实字段、结果回流与副作用边界 |
| Schema 设计与接口评审人员 | [全部 Schema 图册](schema-reference_zh.md) | 查看 22 个文件、14 个逻辑 Schema 及逐份结论 |
| PR Reviewer | [完整架构审查](architecture-review_zh.md) | 查看 P1、设计债务、证据和验证 |
| Agent Host POC 负责人 | [与 Agent Host POC 的对照](poc-comparison_zh.md) | 确认现状、目标和连接层 |

## 交互式架构图

- [Provider 生效架构](provider-effect-architecture.html)：区分当前运行主链、Schema 映射、
  最终权力与产品化缺口。
- [Tokenless 生效时序](provider-effect-sequence.html)：采用 Drafter 工程蓝图风格，使用同一份 `list_recent_builds`
  fixture，贯穿展示真实 artifact id、source digest、scope、359→110 meters、candidate、
  receipt 与最终 replacement。
- [Agent Sec 命令检查时序](security-command-call.html)：用六组简化字段展示 pipe-to-shell
  命令、`shell-download-exec` 结果、Core gate 与 Ledger 事实。
- [Checkpoint 创建时序](checkpoint-create-call.html)：用六组简化字段展示当前 CLI、
  Unix socket、ws-ckpt 与 Btrfs 链路，并标明现有 AW 边界。

三份调用时序图均为自包含 Drafter HTML，默认直接展示字段值和字段语义，并支持按步骤
定位。Provider 生效架构图为自包含 Archify 产物，支持引导视图、节点检索、路径跟踪、
主题切换和导出。14 张浅色 Schema SVG 位于 `images/schemas/`，其确定性数据源与生成脚本
位于 `diagram-sources/`。

Provider 生效架构图通过九项 showcase 校验，错误和警告均为零。全部页面交付前另使用
Firefox headless 进行截图检查。
