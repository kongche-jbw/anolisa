# AW Provider 架构说明

## 结论

这套架构的大方向是对的。它为 Agent OS 增加了一层可替换的系统能力总线。Tokenless、安全扫描、未来的记忆、证据处理和其他系统能力，可以通过同一套 Capability Contract 接入，而不必把每一种实现硬编码进 COSH。

PR 3 已经把核心骨架搭出来，但仍处于源码 POC。它证明了通路能跑，也把主要职责拆开了。它还没有证明安装后会自动生效，也没有达到生产安全边界。

## Provider 到底是什么

这里的 Provider 指 Capability 的实现者。

例如 Context Projection 是一种能力。Tokenless 可以实现它，未来另一个压缩器也可以实现同一个能力。COSH 不需要知道实现者叫 Tokenless，它只需要把 Tool Result 交给 AW，并说明自己需要哪个 Contract 版本。

安全检查也是同样的关系。Security Command Inspect 是能力，Agent Sec 是一个实现。是否拥有阻断权，由 Capability 的 Authority 决定，不由 Provider 自己决定。

现有 Agent Host POC 里的 mock provider 指离线模型 Provider。两处 Provider 不是同一个概念。管理层材料建议统一写成 Capability Provider 和 Model Provider，避免后续讨论混线。

## 一次 Provider 生效需要什么

Provider 生效不是把 manifest 放进目录就结束了。完整链路需要八个条件同时成立。

1. Provider binary 与 manifest、schemas 被同一个受控发布单元安装。
2. AW Host 从明确的绝对根目录发现并准入 manifest。
3. manifest 中的 Capability、Authority、Scope、Contract digest、权限和资源上限通过校验。
4. COSH 在正确的 PreToolUse 或 PostToolUse 边界启用 AW Hook。
5. COSH 提供稳定的 session、turn、tool use 与 attempt 关联。
6. AW Core 的计划和 policy binding 选中了这份 Provider。
7. Host 完成 codec 映射、进程监督、超时和输出校验。
8. Environment 接受最终决策或候选结果，并回报最终采纳事实。

PR 3 已经覆盖第 2、3、5、6、7 项的源码主干，也做了第 4 和第 8 项的局部 adapter。当前发布体系尚未把第 1 和第 4 项做成默认产品行为，第 8 项也缺最终采纳回执。

## 五层职责

| 层次 | 谁负责 | 核心责任 | 不该做什么 |
| --- | --- | --- | --- |
| Agent Environment | COSH、IDE、工作流引擎 | 持有真实 Tool 边界、最终执行和入模决定 | 不按 Provider ID 写业务分支 |
| AW Core | AW | 建 Execution Context、制订计划、精确路由、验证候选 | 不解析 COSH 私有 wire format |
| Provider Host | AW | 发现、准入、codec、预算、进程监督、Receipt | 不决定业务策略 |
| Capability Provider | Tokenless、Agent Sec 等 | 执行自己的算法并返回 native 结果 | 不自行扩大 Authority |
| Ledger | AW | 保存被允许的无正文事实和哈希链 | 不保存 Tool 正文或候选正文 |

这个边界划分值得保留。它让每个团队都能守住自己的权威事实，也方便以后把源码 POC 替换成常驻 AW service。

## 为什么要分 Observe、Advise、Mediate

Observe 只能报告事实。例如某条规则命中、估算 token 数是多少。

Advise 可以给候选。例如建议用一个可逆的压缩表示替换原 Tool Result。最终是否采用仍归 Environment。

Mediate 可以影响工具是否执行。例如 Block、Ask 或 Allow。

三种权力分开后，接入一个统计 Provider 不会意外得到阻断权，接入一个安全 Provider 也不会顺手改写模型上下文。这个设计是本 PR 最重要的治理价值之一。

## 与 Agent Host POC 怎么拼

Agent Host POC 已经有主机生命周期、签名 RPM、镜像、systemd、Gateway Task 和 Run、workspace checkpoint、AgentSight、Agent Sec 健康以及 `anolisa host verify`。

AW 应放在这套主机基座之上，成为每次 Tool Call 的能力面。理想形态是由 system scope 的 AW service 持有 Provider catalog、policy binding、Ledger writer 与健康状态。COSH 只通过稳定客户端协议调用它。

`anolisa top` 和 `host verify` 应新增四类事实。

- AW service 是否 ready
- 哪些 Capability Provider 已准入并可路由
- COSH 的 AW Hook 是否真的加载
- Ledger writer 和 final adoption 回执是否健康

这样才不会出现 Agent Sec daemon healthy，但 Tool Call 实际没有经过安全 mediation 的假象。

## 当前风险怎么理解

当前风险不代表架构要推倒重来。大部分问题集中在边界没有闭合。

- Hook 扫描的脱敏副本与实际执行原文不一致
- Ledger 的哈希没有覆盖所有查询事实
- content-free 仍靠 key 黑名单
- Provider 后台进程与 codec 内存预算没有完全收住
- Agent Sec 的 auto 语言可能漏检 Python
- PostToolUse 外层失败会静默回到原始结果
- CI 和真实二进制链路没有给出可信门禁
- AW 没有正式安装与默认激活路径

这些问题修好后，Contracts、Core、Host、Adapter、Ledger 的分层仍可以沿用。

## 建议管理决策

本 PR 先按 source POC 收敛，修完 P1 后再合并。PR 描述要清楚写出当前不会随安装自动生效。

下一阶段单独立项完成产品闭环，包括统一 Provider root、原子包、AW service、Hook 激活、policy binding、状态投影、真实 Provider 集成测试和 KVM 验收。

安全生产化还要补 OS sandbox 或等价 containment、executable identity、外部锚定 Ledger，以及 Environment 最终采纳回执。
