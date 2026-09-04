# PR 3 与 Agent Host POC 对照

配套的[当前 Checkpoint 创建时序图](checkpoint-create-call.html)展示 COSH 到 ws-ckpt 的
现行实线链路；[运行实例与边界说明](runtime-call-examples_zh.md)进一步区分当前实现与未来
AW Enforce/Reconcile 目标。

## 两边各自在解决什么

Agent Host POC 解决主机与交付生命周期。它关心镜像如何构建、RPM 是否签名、服务是否启动、任务是否有稳定状态、workspace 如何 checkpoint、AgentSight 与 Agent Sec 是否健康，以及 `anolisa host verify` 能否给出可信结论。

PR 3 解决一次 Tool Call 内的能力协作。它关心 Environment 如何表达上下文，Core 如何制订 Capability Plan，Host 如何找到并执行实现，Provider 如何返回观察或候选，Ledger 如何记录边界事实。

两者没有方向冲突。前者是主机控制面和交付面，后者是 Tool Call 能力面。现在欠缺的是把能力面变成主机里可安装、可激活、可观察的一等服务。

## 对照表

| 维度 | PR 3 | 当前 Agent Host POC | 判断 |
| --- | --- | --- | --- |
| 入口 | COSH built-in PreToolUse 与 PostToolUse | COSH 登录、Gateway Task 和 Run、主机命令 | COSH 作为默认 Environment 的方向一致 |
| 运行单元 | 每个 Hook 进程内创建 Core 与 Host，Provider one-shot | systemd 服务、KVM、签名 RPM 闭包 | PR 仍是临时形态，未来应进入 system-scope AW service |
| 状态权威 | Core 持有 Plan 和 policy，Host 持有 admission 和 invocation | Gateway 持有 Task 和 Run，各组件持有自己的真实状态 | 权威分层一致，但 POC 尚未登记 AW 的新事实 |
| 安全 | Mediate 支持 Ask、Block、Allow，Provider 声明尚未由 sandbox 强制 | Host verify 缺项即失败，Agent Sec 只表述 V1 health | 原则相容，PR 还不能宣称生产安全隔离 |
| 审计 | Hook 侧过渡 Ledger，保存 Receipt 与边界摘要 | 主机状态、组件日志、Task 和 Run 投影 | 需要 system writer、完整哈希承诺和最终 adoption |
| 发布 | 手工源码构建与绝对路径参数 | reviewed signed exact RPM、镜像、KVM | 当前最大断层 |
| 可观测性 | Graph 和 Receipt 只在开发命令中可见 | `anolisa top` 与 `host verify` 有统一入口 | 必须新增 AW service、graph、hook、ledger 状态 |
| 故障语义 | Provider failure、gap、gate degradation | unknown、degraded、not ready 不伪装 healthy | 设计语言一致，Graph 实现还要补逐包失败隔离 |
| Provider 术语 | Capability 实现 | POC mock provider 是离线模型实现 | 必须改成 Capability Provider 与 Model Provider |
| Agent Sec V2 | Python one-shot bridge | POC 明确只验证 V1 health | 应标成 interim bridge，不能替代 V2 daemon 计划 |

## 需要补上的连接层

### 发布连接

统一 Provider FHS root 为 `/usr/share/aw/providers`。每个组件的 binary、manifest、schemas 和版本证明进入同一原子包。AW 自身增加 component manifest、raw/RPM 产物与安装回滚。

镜像构建继续使用现有 signed exact RPM closure。Host admission 的 manifest digest 只能证明 contract 文件，不能证明实际 executable。包事务、版本握手或 executable digest 需要补一层。

### 运行连接

增加 system-scope AW service。它持有 Provider catalog、policy binding、state root、Ledger writer 和服务级资源治理。COSH Hook 通过稳定 client contract 调用，不再由每个短命 Hook 进程各自持有 SQLite 和 catalog。

Provider 的 OS sandbox、cgroup 或等价 containment 在这一层落实。当前 `--allow-unenforced-provider` 只用于 source POC 和诊断。

### 状态连接

主机状态模型至少增加以下事实。

- `aw_service_state`
- `provider_catalog_revision`
- `capability_graph_health`
- `cosh_aw_hook_loaded`
- `ledger_writer_state`
- `last_verified_chain_tip`
- `final_adoption_observed`

Graph 必须保留每个 package 的准入失败，不应因为一个可选 Provider 损坏就让整个 catalog 消失。`host verify` 要检查 Capability 是否可路由，不能只检查 manifest 文件存在。

### 关联连接

COSH 已经为 session、turn、tool use 和 attempt 建立稳定关联。下一步要让 Gateway 的 Task 和 Run、AW invocation、Provider Receipt、Ledger event、AgentSight trace 和 ws-ckpt snapshot 可以用受信任映射关联。

这些 ID 只承担归因，不能承担授权。授权仍由 system service 的 peer identity 和 policy 生成。

### 最终结果连接

PostToolUse 的 `replacement_requested` 只说明 AW adapter 提出了候选。后续 Hook 可能覆盖，Block 也可能阻止结果进入 history。Environment 需要在聚合完成后写一条 final adoption fact。

只有拿到这条事实，Gateway 和 Ledger 才能区分候选已生成、候选已请求、候选最终进入模型三种状态。

## 分阶段演进建议

### 阶段一 修好 source POC

修复 Hook 字节一致性、Agent Sec auto、Ledger 承诺范围、content-free 类型、one-shot 清理、codec 预算、默认测试竞态和 CI diff 范围。补两个真实 Provider 的非 ignored smoke。

### 阶段二 进入可安装镜像

统一 Provider root，交付 AW package 和 system service，注册 COSH Hook，接入 policy binding、`anolisa top` 与 `host verify`。在现有 KVM POC 中验证 cold boot、升级、回滚和部分 Provider 损坏。

### 阶段三 完成生产安全边界

加入 OS containment、executable identity、受信任 principal、Ledger 外部锚定、retention、增量 verify、final adoption 和跨组件 trace。Agent Sec 能力逐步迁入 V2 Rust daemon contract。

## 最终判断

老板整理的主线可以确认。稳定 Capability Contract、通用 Host、策略 Core、Environment Adapter 和无正文 Ledger 这五个方向都值得继续。

需要修正的地方主要是成熟度判断。PR 3 已经证明架构骨架和源码调用链，尚未证明产品安装链、生产安全边界和审计可信闭环。把这三层分别立项，能保留架构优势，也能防止 POC 的成功被误读成生产能力已经上线。
