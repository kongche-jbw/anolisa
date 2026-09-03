# AW Provider 组件接入手册

这份手册面向准备把组件能力接入 AW 的开发同学。当前 PR 提供的是 source POC contract。下面的流程同时标出源码接入要求和未来产品化要求。

## 先判断你的能力属于哪一种 Authority

| Authority | 能做什么 | 典型例子 | 失败时的默认处理 |
| --- | --- | --- | --- |
| Observe | 报告事实，不改变执行和模型内容 | 风险命中、token 估算、证据分类 | 记录 gap，主流程继续 |
| Advise | 给出候选，Environment 决定是否采用 | 可逆 Context Projection | 没有候选时保留原文 |
| Mediate | 影响 Tool Call 是否执行 | Allow、Ask、Block | 按明确 failure policy 处理 |

不要让 Provider 自己把 Observe 结果解释成阻断。Authority 由 canonical Capability Contract 与 Core Plan 固定。

## 责任边界

AW 团队负责 canonical Capability schema、Contract version、Core 路由语义、Host 准入与执行、通用 Receipt 和 Ledger contract。

组件团队负责 native endpoint、native request 和 response schema、算法语义、运行资源需求、权限声明、版本一致性、组件自身的无副作用证明。

COSH 或其他 Environment 团队负责真实边界、原始输入与执行输入一致性、最终候选采纳、用户交互、Hook 聚合和最终 adoption 回执。

发布团队负责把 binary、manifest、schemas 和版本证明做成原子安装单元，并把 AW runtime、Hook 和状态检查接入镜像。

## Provider package 应包含什么

建议目录保持为下列形态。

```text
providers/<provider-id>/
├── provider.toml
├── schemas/
│   ├── canonical-input.schema.json
│   ├── canonical-output.schema.json
│   ├── native-request.schema.json
│   └── native-response.schema.json
├── fixtures/
└── README.md
```

`provider.toml` 至少需要声明 Provider identity、version、executable、每项 Capability 的 Authority 与 Scope、canonical Contract identity 和 digest、native schema digest、json-map codec、timeout、input 与 output bytes、环境变量、网络、文件系统和持久化要求。

所有 digest 必须由测试从仓库文件重算。组件版本变化时，manifest version、组件 manifest 和包版本要原子更新。

## Native endpoint contract

当前 exec-json/v1 的进程协议应保持简单。

- stdin 读取一个完整 JSON document，读到 EOF 后再处理
- stdout 只写一个 JSON document
- settled 业务结果以退出码 0 返回
- crash、协议损坏和不可恢复基础设施错误使用非零退出码
- stderr 不得被当作用户错误正文或审计正文持久化
- 不依赖 ambient PATH、HOME 或继承环境
- 不在 Provider 路径重复写入 Tool 正文、SecurityEvent 或 telemetry
- 明确最大输入、最大输出、最长耗时和是否需要 state directory

若 Provider 需要 state directory，当前 Core 路径还不能正确提供。请先与 AW 团队补齐 contract，不要只在 manifest 中声明后假设可用。

## Canonical 与 native 数据如何连接

json-map/v1 只做声明式字段映射。mapping 不应根据 provider_id 分支，也不应在 Host 里加入某个组件专属逻辑。

canonical schema 与 Rust 公共类型必须共享 conformance vectors。特别检查 ASCII 约束、UTF-8 bytes 与 Unicode 字符数、ID pattern、未知字段、空字符串和最大数组长度。只验证 schema 文件能解析和 digest 正确还不够。

Provider 输出里的 meter method、media type、transform chain 和 rule ID 都可能进入 Receipt 或 Ledger。它们应使用受限词表或专用标识类型，不能成为回传正文的自由文本口袋。

## 生效链路

组件完成 Provider package 后，仍需要完成以下产品接线。

1. 发布包把 binary、manifest 和 schemas 安装到统一的 `/usr/share/aw/providers/<id>`。
2. AW service 或 Host 从统一根目录逐包准入，并输出真实健康原因。
3. policy binding 把具体环境和 Capability 绑定到已准入实现。
4. COSH 配置启用 AW PreToolUse 或 PostToolUse Hook。
5. Hook 与 Environment 传递同一份将执行或将入模的数据，并附带稳定 correlation。
6. Core 解析 Plan，Host 执行 Provider，Environment 接受决策或候选。
7. Ledger writer 记录受限事实，Environment 再写最终 adoption 状态。
8. `anolisa top` 与 `host verify` 展示 service、graph、hook、ledger 和 adoption 健康。

缺少其中任一步，都只能说 Provider 已安装或已准入，不能说 Provider 已生效。

## 必须有的测试

### 组件侧

- native request 和 response 的正例、边界值与 malformed 输入
- timeout、oversize、非 UTF-8、非零退出和 stderr 含 secret
- no-network、no-filesystem、no-retention 等声明的快照或隔离证明
- manifest digest、component version 与 package inventory 一致
- auto 或默认值在每一种支持语言和媒体类型上的行为

### AW 侧

- doctor 能逐 Provider 展示 Ready、Degraded、Unavailable 与原因
- real binary 加 real manifest 加 real Host/Core 的非 ignored smoke
- mapping complexity 与重复大字段不能越过内存预算
- Provider 主进程成功退出后，后台子进程仍会被清理
- schema validator 和 Rust deserializer 对同一 conformance vectors 给出一致结果
- Observe fan-out 保留 invocation order、provider identity 和 settled gap reason
- Ledger 拒绝未知 kind、错误 schema 和所有自由文本泄漏通道
- 篡改 header 或 scope 后 verify 必须失败

### Environment 与发布侧

- Hook 实际收到的输入与工具即将执行的输入一致
- Projection 的 source bytes 与候选替代目标一致
- 多 Hook 聚合后有最终 adoption 回执
- raw、RPM 和镜像里的 installed graph 可以发现两个真实 Provider
- 升级、部分失败和回滚不会产生 manifest 与 binary 版本错配
- KVM 启动后 `host verify` 能发现 Hook 未加载、Provider 不可用和 Ledger writer 失败

## 当前两个示例 Provider 的特别注意事项

Tokenless 已经有 raw 与 RPM Provider 资源，但 AW runtime 和 Hook 没有随之安装。它的 native `agent_id` 表示 frontend 名称，不能直接用 environment instance id 代替。启用 stats 或 SLS 前要先修正归因语义。

Agent Sec 的 Provider 资源目前没有进入任何正式 package。它的 auto 语言会把普通 Python 输入送入 Bash scanner。现有内置规则又以 warn 为主，所以架构支持 Block 不等于当前默认规则会真实阻断。

Agent Sec V2 目标是 system-scope Rust daemon。当前 Python one-shot Provider 应标成 interim V1 bridge。新的能力不要继续依赖 Python 进程形态，除非迁移合同明确要求。

## 接入评审清单

- Capability identity、Authority、Scope 和 Contract version 已由 AW 团队确认
- Provider package 没有 provider_id 专属 Host 或 Core 分支
- canonical 与 native schema、digest、fixture、Rust 类型一致
- binary 与 manifest 在同一原子包中，安装根目录统一
- 权限和资源上限由运行时强制，无法强制的部分明确标为 declared_not_enforced
- real Host/Core smoke 在 CI 默认运行，没有 ignore
- Provider absence、failure、timeout 和 malformed output 都有可观测 gap 或 gate degradation
- Receipt 和 Ledger 没有正文与 covert channel
- Hook 生效、最终采纳和回滚都能从主机状态面看到
