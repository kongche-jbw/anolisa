# PR 3 架构审查总报告

审查对象为 casparant/anolisa 的 PR 3。固定基线为 `8574ecb022ec9ffc68e1a71e30f2186b6ec81674`，固定头提交为 `42d07649409ecd5bb023056b28545efbd9325ef2`。审查采用 merge-base 到 head 的完整差异，共 158 个文件，新增 22257 行，删除 620 行。

## 一句话结论

架构方向值得保留。Contracts、Provider Host、Core、Environment Adapter、Ledger 的职责划分清楚，也与现有 Agent Host POC 的权威边界相容。

当前版本仍不适合合并，更不能对外表述为 Provider 已经在产品里生效。这里同时存在运行语义错误、Ledger 可信度缺口、进程与资源边界缺口、测试门禁失真，以及安装激活链路未完成的问题。

建议结论为 Request changes。先修完下列 P1 项，再讨论 source POC 合并。产品化工作应另列里程碑，不能混在一句已经接入里带过。

## 做对了什么

这套设计最有价值的地方，是把能力名称与具体实现拆开。Environment 只提交规范化 Tool Call 或 Tool Result，Core 只根据 Capability、Authority、Scope、Contract digest 和运行状态做计划，Host 负责 manifest 准入、codec 和进程执行。具体组件保留自己的 native protocol。

这会带来三项长期收益。

- 新 Provider 可以在不改 Core 分支逻辑的情况下替换同一 Capability。
- Observe、Advise、Mediate 三种权力分开，安全扫描不会因为顺手做了摘要就得到阻断权。
- 候选内容与 Receipt 分开，后续可以保留操作证据，同时避免重复保存完整 Tool 内容。

显式 manifest root、不查找环境 PATH、精确 schema digest 路由、未强制的 Provider 必须显式 opt-in，这些选择也很稳健。PR 对缺少 OS sandbox、缺少常驻 writer、缺少最终采纳回执等限制写得比较诚实。

## 合并阻断项

### P1 Hook 处理的字节与实际执行和入模的字节不一致

COSH 在调用所有 Hook 前，会对整个 HookInput 做 secret redaction。AW 的 PreToolUse 安全检查和 PostToolUse 摘要因此拿到脱敏副本。随后工具仍可能执行原始参数，模型在没有替代项时仍会收到原始 Tool Result。

影响很直接。安全 Provider 可能对脱敏后的命令判定安全，但 COSH 执行的是另一串原文。Tokenless 也可能对脱敏副本做所谓 lossless 变换，而 lossless 只相对脱敏副本成立。Ledger 中的 digest 和安全结论也会指向错误的字节集合。

证据见 `src/cosh-ng/crates/cosh-core/src/hook.rs` 的 `run_hooks`，`src/cosh-ng/crates/cosh-core/src/redaction.rs` 的递归字符串改写，`src/cosh-ng/crates/cosh-core/src/core.rs` 的原始结果回填，以及 `src/aw/crates/aw-cosh-hook/src/lib.rs` 的内容提取。

最小修正需要定义一条明确契约。用于 Mediate 的数据必须与将要执行的输入一致。用于 Context Projection 的 source artifact 必须与最终候选要替代的模型可见内容一致。若模型可见面本来就要求脱敏，应在 Environment 内先完成受信任的内容变换，并让后续原始分支也使用同一份值。

### P1 Agent Sec 的 auto 语言会漏掉普通 Python 风险

公共 schema 允许 `language=auto`。manifest 原样把它传给 Agent Sec。实现仅在语言显式为 Python 时选 Python scanner，其他值都走 Bash scanner。

普通 Python 文件里的 `pickle.loads` 一类风险因此可能得到假 clean。现有 auto 测试只覆盖 Bash 文本，没有覆盖只会命中 Python 规则的输入。

最小修正有两个可选方向。V1 Contract 移除 auto 并要求 adapter 明确语言，或者实现可靠检测并增加 Python-only、Bash-only 与模糊输入回归。安全边界里不宜保留含糊的默认猜测。

### P1 Ledger 的哈希验证没有覆盖读路径使用的全部事实

`verify_chain` 会重算 body digest、record digest 和 parent 链，但不会把 SQLite 的 `kind`、`schema`、`timestamp_ms` 等列与 `record_canonical` 内的 header 逐项比对。`ledger_scope` 也完全不在哈希承诺里。

攻击者可以修改 kind、schema、时间或 attempt、tool_use、invocation 关联，查询接口会展示修改后的值，verify 仍可能通过。这与 tamper-evident record 的表述不一致。

最小修正应从 `record_canonical` 解出严格 envelope，重新规范化编码，并与每一个持久化 header 列比对。审计归属所需的 scope 也应进入被承诺字节，或者明确降级为可重建、可丢弃的索引缓存。损坏数据库是预期威胁面，查询 API 不应使用 `expect` 直接 panic。

### P1 content-free 目前靠黑名单，无法形成安全证明

Ledger admission 只禁止六个 key，`LedgerSink::record` 仍接受任意 kind、任意非空 schema 和任意 `serde_json::Value`。使用 `note`、`details`、`text` 等字段即可写入正文或 secret。

Provider 自由文本还有更隐蔽的通道。meter method、media type、transform chain 等字段可从 native response 进入 Receipt 或 Ledger。`BoundedName` 只限制非空、NUL 和字节数，无法约束语义。

最小修正应把可入账事件改为封闭的 typed enum，建立 kind、schema 与严格 body 类型的一一对应。body 类型启用未知字段拒绝。meter method、media type 和 transform identifier 使用专用类型或准入注册表。可持久化 metadata 尽量来自 admitted manifest。

### P1 one-shot 进程仍可留下后台后代

Provider 主进程输出合法 JSON 并正常退出后，Host 不会在成功路径清理进程组。关闭标准流后继续运行的后台进程可以留下。`setsid` 后代在超时路径也能逃出原进程组。

最小修正应覆盖每一个 terminal path 的清理，并新增成功响应后后台化与 `setsid` 逃逸测试。生产级监督需要 cgroup、PID namespace 或 subreaper。单靠 process group 只能提供较弱保证。

### P1 codec 在限额检查前存在可控内存放大

请求和响应 mapping 都会 clone JSON value。manifest 没有字段数量上限，input、output 和 deadline 检查又发生在完整映射和编码之后。一个大字段被大量 mapping 重复引用时，可以在限额报错之前消耗巨量内存。

最小修正应先检查绝对 deadline，对 manifest complexity 设上限，并在增量 mapping 过程中按实际分配或编码预算即时终止。

### P1 CI 只看最后一个提交，新增门禁在本 PR 中几乎没有运行

主 CI 使用 `fetch-depth=2`，再比较 `HEAD~1..HEAD`。当前最后一笔提交只改一个 Agent Sec schema 文件，前面十六个提交里的 AW、COSH、Tokenless 和 Provider 变化不会进入组件检测。

同时，`providers/agent-sec-core/**` 既不会触发 Agent Sec，也不会触发 AW。AW job 本身写得完整，但变化检测可以绕开它。

最小修正应在 pull_request 上获取 declared base，比较 `base...head`。Agent Sec Provider 路径要同时触发 Agent Sec 与 AW，并为多提交 PR、provider-only 变化增加检测回归。

### P1 默认 AW 测试存在并行竞态

`cargo test --workspace --locked` 在默认并行模式下不稳定。一次运行出现 deny 被降为 Ask，另一次出现三个失败。单测串行和单个测试重复运行可以通过。

根因是 shell fixture 不读取 stdin 就退出。Host 的并发 stdin writer 因 BrokenPipe 把本来已产出合法响应的 Provider 标为失败。测试结果随调度变化。

这里应修 fixture，让它完整消费 stdin，再输出响应。另加一个专门覆盖早退 Provider 的 Host 回归。不能通过放宽 Host 错误处理来掩盖协议不完整。

### P1 PostToolUse 的外层 Hook 失败被静默吞掉

COSH 会把 spawn、timeout、非零退出和非法 JSON 记进 `HookExecution.failure`。PreToolUse 聚合器会读取它，并按 fail policy 阻断或通知。PostToolUse 聚合器没有读取这个字段，只拿默认空 output 继续。

AW Core、Provider Host 或 required Ledger 失败时，`aw-cosh-hook` 会非零退出。COSH 随后把原始 Tool Result 送给模型，既没有用户或运维通知，也没有 ObservationGap 或 Ledger 记录。观察系统不可用和观察系统没有发现因此变成同一种外观。

最小修正应让 PostToolUse result 与 PreToolUse 对称地携带 hook failures。Context transformation 可以继续 fail-open，但必须输出稳定的 operator notification 和 content-free audit gap。required profile 还要约束 COSH 外层 `fail_open`，避免两层策略互相抵消。

## 明显疏漏与设计债务

### Source POC 没有可安装的激活闭环

`scripts/build-all.sh --dry-run --component aw` 会直接报 Unknown component。AW 没有 `.anolisa/component.toml`、RPM、raw package、systemd 或默认 Hook 配置。Tokenless 包会带 `/usr/share/aw/providers/tokenless`，但 Agent Sec 文档却写 `/usr/share/agent-workload/providers/agent-sec-core`，而正式 Agent Sec 包根本没有携带这份 manifest 和 schemas。

所以安装 Tokenless 或 Agent Sec 不会让 AW Provider 自动生效。当前唯一可运行方式是源码构建二进制，再手工传绝对 manifest、executable root 和 opt-in 参数。

如果这次只想合入 source POC，PR 标题、描述和组件文档应明确说明不可安装、不会自动生效。若目标是进入 Agent Host POC，则需要统一 Provider root，并补齐 AW package、服务、Hook 注册、policy binding、健康检查、回滚和镜像验收。

### Capability Graph 的健康状态大多不可达

目录 discovery 对整个集合 fail-fast。任一 Provider package 损坏就没有 catalog。成功入图的条目又总是 Ready。Installed、Admitted、Degraded、Unavailable 和 reason 无法真实表达逐 Provider 状态。

更合理的做法是逐 package admission，保留失败条目与原因，Core 只路由 Ready。若团队希望坚持 catalog 原子准入，也应把这个故障域写进 contract，避免运维误以为可选 Observe Provider 的损坏不会影响 Mediate。

### Observe 失败丢失归因与顺序

Core 把 observations 和 gaps 放在两个数组里，最后再拼 receipts。多 Provider 交错调用时，原始 invocation order 无法重建。invoke error 转成 gap 时还会丢失 provider、invocation 和具体 settled failure。

建议用一个有序的 step result enum 保存每个计划步骤的 produced 或 gap，再从中派生 observations、candidate 和展示视图。

### Observe 被强制依赖 Advise

PostToolUse Plan 要求 projection `ExactlyOne`，Core 又在执行任何步骤前解析完整计划。只安装 Agent Sec Observe 而没有 Tokenless Advise 时，整个计划会在采集观察前失败。projection 执行或校验错误也会通过 `?` 丢掉已经完成的 Observe 事实。

这是一项可以接受但必须明说的取舍。当前实现选择全计划路由快照，代价是能力无法独立部署和部分成功事实被丢弃。若产品希望安全观察在摘要 Provider 缺失时仍工作，Core 应返回 partial plan outcome，并把 projection 失败表达为 gap。

### Schema 与 Rust 类型并不等价

JSON Schema 的 `maxLength` 按 Unicode code point，Rust `BoundedName` 按 UTF-8 bytes。tool_name 的 schema 允许 256 个字符，Rust 公共类型只接受 128 bytes。source artifact id 的 schema 也没有表达 Rust canonical ID pattern。

这会产生 schema-valid 但 Rust-contract-invalid 的跨语言结果。需要共享 conformance vectors，并统一字符集、长度单位和 ID pattern。

### 声称源码解耦，实际仍直接 path dependency

提交说明称 COSH Contracts 通过 vendoring 与 AW workspace 解耦。当前源码态 `cosh-gateway-contracts` 和 `cosh-core` 都直接 path-depend `aw-contracts`，只有 source-package 阶段临时复制并改写路径。

应先修正文档中的架构表述。若组件独立是硬约束，应发布版本化 contract crate，或真正维护 generated snapshot。`cosh-core` 也应只经过自己的 contract facade。

### 实际二进制没有绑定 manifest identity

Receipt 的 provider_version 来自 manifest。Host 没有 executable digest，也没有版本握手。手工 root 或部分升级场景中，可以执行一份二进制，却记录另一份版本。

POC 的 signed exact RPM closure 能降低风险，但 source 路径仍没有证据。产品化时应采用原子包加 doctor version check，或把 build identity 和 artifact digest 纳入 admission。

### 外层和内层容量上限不一致

AW 默认允许 64 MiB Provider output，COSH Hook pipe 只允许 32 MiB。32 到 64 MiB 之间的 AW 合法结果会被外层丢弃。Observe fan-out 目前还是逐个同步执行，每个 Provider 都拿到新的完整 deadline，没有 plan 总预算，Provider 数量增长时尾延迟线性增加。

Ledger 所谓 bounded query 只保证走索引，没有 SQL LIMIT 或分页。便利命令还会加载所有 kind 后在内存排序。应把 bounded 明确成索引边界、返回行数边界和内存边界三件事，不能只满足第一件。

### ArtifactId 的重试稳定性只在单个 Core 生命周期内成立

ArtifactId 包含随机 execution context。COSH 每次构造 Core 都会生成新的 context。进程重启后，相同 session、turn 和 tool call 的 ArtifactId 会变化，但文档当前把它描述为稳定重试身份。

要么持久化 execution context，要么把稳定性承诺收窄到同一 Core 生命周期。idempotency key 与 ArtifactId 的输入集合也应保持可解释的一致关系。

### 新组件登记仍不完整

根 README 没有 AW 组件行。PR 标题仍是分支名风格，PR body 还是空模板，没有 issue、风险、验证和回滚说明。对一份跨 158 个文件的安全敏感架构 PR，这会显著增加误读成本。

## 与现有 POC 的关系

现有 POC 主要解决主机与交付面，包括签名 RPM 闭包、镜像构建、KVM 启动、systemd、Gateway Task 和 Run、workspace checkpoint、AgentSight、Agent Sec 健康与 `anolisa host verify`。

PR 3 主要解决一次 Tool Call 内的能力面，包括规范化 Contracts、Capability Plan、Provider discovery、codec、单次调用、候选结果、Receipt 与过渡 Ledger。

两者处在不同层次，方向可以拼接。当前缺少的是连接层。AW 的服务状态、Provider Graph、Hook 生效状态和 Ledger 健康还没有进入 `anolisa top` 与 `host verify`。Agent Sec daemon 可以显示 healthy，而 AW mediation 其实完全没加载。现有 POC 的 mock provider 指离线模型 Provider，PR 中的 Provider 指 Capability 实现，文档必须持续区分。

## 建议的收敛顺序

1. 修复 Hook 字节一致性、Agent Sec auto、Ledger 承诺范围、content-free 类型封闭、one-shot 清理和 codec 预算。
2. 修复默认 AW 测试与 CI diff 范围，加入两个真实 Provider 的非 ignored Host/Core 链路。
3. 明确本 PR 定位。若是 source POC，删去安装已完成的暗示。若是产品集成，统一 FHS root 并补齐 AW 安装激活闭环。
4. 把 Graph 的失败隔离、Observe 有序结果、schema conformance 与 executable identity 作为下一阶段架构任务。
5. 在 Agent Host POC 中增加 AW service、Hook、Provider Graph、Ledger 和 final adoption 的状态投影，再做 signed image 与 KVM 验收。

## 验证记录

已通过的检查包括 AW format、clippy、doc，Agent Sec AW Provider 单元与 E2E 共 27 项，COSH scope 相关测试，版本一致性、manifest digest、schema fixture 和 `git diff --check`。

未通过的检查为 AW workspace 默认并行测试。真实 Tokenless Host、Core 与 COSH 测试目前均被 ignore。GitHub 上除 Pages build 外，相关检查自 2026 年 9 月 3 日起一直处于 queued，不能视为 CI 通过。

PR 对自己的 fork main 显示 mergeable，但 declared base 比当前 `up/main` 落后 83 个提交。对当前 `up/main` 做 merge-tree 已在 Tokenless CLI 的 `main.rs` 与 `cli_integration.rs` 产生冲突。
