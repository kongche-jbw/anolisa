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
| Schema 设计与接口评审人员 | [全部 Schema 图册](schema-reference_zh.md) | 查看 22 个文件、14 个逻辑 Schema 及逐份结论 |
| PR Reviewer | [完整架构审查](architecture-review_zh.md) | 查看 P1、设计债务、证据和验证 |
| Agent Host POC 负责人 | [与 Agent Host POC 的对照](poc-comparison_zh.md) | 确认现状、目标和连接层 |

## 交互式架构图

- [Provider 生效架构](provider-effect-architecture.html)：区分当前运行主链、Schema 映射、
  最终权力与产品化缺口。
- [Tokenless 生效时序](provider-effect-sequence.html)：沿调用顺序展示 canonical input、
  native request、native response、candidate、receipt 与最终 replacement。

两份 HTML 均为自包含 Archify 产物，默认采用浅色 editorial 风格，支持引导视图、节点检索、
路径跟踪、主题切换和导出。14 张浅色 Schema SVG 位于 `images/schemas/`，其确定性数据源
与生成脚本位于 `diagram-sources/`。

架构图和时序图均通过九项 showcase 校验，错误和警告均为零。审查环境没有 Chrome 或
Chromium，Archify 内置浏览器复核会报告跳过；交付前另使用 Firefox headless 对浅色页面
进行人工截图检查。
