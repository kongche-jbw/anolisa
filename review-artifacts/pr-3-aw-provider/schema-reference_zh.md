# PR 3 Schema 参考与语义讨论

本文面向第一次接触 AW Provider 的研发、测试和架构评审人员。文档说明 PR 3 中每份
Schema 的字段职责，区分原 PR 的风险、fork PoC 已验证的修复，以及接口冻结前仍需由
架构负责人决定的语义。

原 PR 审查基线为 `8574ecb022ec9ffc68e1a71e30f2186b6ec81674`，头提交为
`42d07649409ecd5bb023056b28545efbd9325ef2`。PoC 对照分支为
[`feat/aw/provider-e2e-poc`](https://github.com/kongche-jbw/anolisa/tree/feat/aw/provider-e2e-poc)。
PoC 是验证修复方向的实现基线，不改变对原 PR 头提交的 Review 结论。

## 1. Schema 在 Provider 架构中的位置

PR 3 涉及 22 个物理 JSON Schema 文件。8 份 AW canonical Schema 在 Provider 包中各有
一份逐字节副本。按内容和语义去重后，共有 14 份逻辑 Schema。

一次无副作用 Provider 调用经过下面的数据边界。

```text
COSH 事件
  -> AW Core 生成 canonical input
  -> Provider Host 校验 canonical input
  -> manifest mapping 生成 native request
  -> Provider 组件返回 native response
  -> Provider Host 映射并校验 canonical output
  -> Provider Host 把 transient output 和 content-free receipt 返回 AW Core
  -> AW Core 解释结果并生成 candidate 或安全事实
  -> AW Ledger 保存完整 post_tool_use_plan
  -> COSH 决定是否把 candidate 写入最终 Tool Result
  -> AW Ledger 保存引用同一 plan 的 context_adoption
```

其中有三个容易混淆的对象。

| 对象 | 是否含正文 | 语义 |
| --- | --- | --- |
| `candidate` | 是 | Provider 建议使用的结果，还不是最终采用事实 |
| `receipt` | 否 | 一次 Provider 调用的输入身份、终态、输出摘要和计量事实 |
| Ledger record | 否 | Plan 记录安全事实与缺口；adoption 记录 COSH 本地采用，两者都不保存正文 |

三层合同的职责如下。

| 合同层 | 维护方 | 解决的问题 |
| --- | --- | --- |
| AW canonical | AW 团队 | 跨组件统一能力名称、输入和输出语义 |
| Provider native | 组件团队 | 保留 Tokenless 或 Agent Sec 自己的进程协议 |
| manifest mapping | Provider 接入方 | 声明 canonical 字段如何变成 native 字段，以及如何映射结果 |

这种分层方向合理。Schema 可以作为改造中心，但不能单独承担全部正确性。JSON Schema
负责字段形状，Rust 或 Python typed validation 负责跨字段不变量，COSH 与 AW Core 负责
证明被检查、被压缩和最终采用的是哪一段字节。

## 2. 原 PR 与 PoC 的阅读口径

下面 14 张图把原 PR 头提交 `42d07649` 与 fork PoC `5ebfc0b3` 放在同一张
字段图中对照。字段区展示 PoC 修正后的可实施形状；绿色区说明已验证的合理点，
橙色区保留冻结 v1 前仍需讨论的边界。每张图下方的“原 PR 风险”再说明原实现
为什么不能直接作为基线。每节采用同一结构。

| 小节 | 含义 |
| --- | --- |
| 语义职责 | 字段应该表达的业务事实 |
| 原 PR 风险 | `42d07649` 上合同与实现不一致的地方 |
| PoC 基线如何解决 | fork 分支中已有提交和测试覆盖的修复 |
| 仍需架构决策 | PoC 没有替团队冻结的长期接口 |

PoC 中与 Schema 直接相关的提交如下。

| 提交 | 已验证的边界 |
| --- | --- |
| [`4d47593b`](https://github.com/kongche-jbw/anolisa/commit/4d47593b) | `BoundedName`、canonical ID 和安全输出状态不变量 |
| [`6238842a`](https://github.com/kongche-jbw/anolisa/commit/6238842a) | 四阶段 JSON Schema instance validation |
| [`d1d813d5`](https://github.com/kongche-jbw/anolisa/commit/d1d813d5) | Agent Sec 判别联合、`auto` 双扫描和失败终态 |
| [`414998b2`](https://github.com/kongche-jbw/anolisa/commit/414998b2) | Tokenless 精确 source fidelity 和响应状态校验 |
| [`1328cf30`](https://github.com/kongche-jbw/anolisa/commit/1328cf30) | Receipt 输入 Schema、输入摘要和输出身份校验 |
| [`5d7f6b62`](https://github.com/kongche-jbw/anolisa/commit/5d7f6b62) | Ledger trace scope、调用来源和 content-free 持久化 |
| [`8ecb1412`](https://github.com/kongche-jbw/anolisa/commit/8ecb1412) | 扫描覆盖、媒体保真、Host 资源边界和 Ledger 状态闭包 |
| [`601b5558`](https://github.com/kongche-jbw/anolisa/commit/601b5558) | COSH effective bytes、最终采用证据和 Checkpoint State Provider |

## 3. 十四份 Schema 快速索引

| 逻辑 Schema | 类型 | 主要用途 |
| --- | --- | --- |
| `context.projection.prepare.input/v1` | AW canonical | 提交即将进入模型的 Tool Result |
| `context.projection.prepare.output/v1` | AW canonical | 返回由 Core 校验、供 COSH 选择的上下文候选 |
| `security.content.inspect.input/v1` | AW canonical | 提交模型可见内容进行敏感信息扫描 |
| `security.content.inspect.output/v1` | AW canonical | 返回内容扫描事实 |
| `security.code.inspect.input/v1` | AW canonical | 提交代码文本和语言策略 |
| `security.code.inspect.output/v1` | AW canonical | 返回代码风险和实际分析语言 |
| `security.command.inspect.input/v1` | AW canonical | 提交执行前命令进行门禁检查 |
| `security.command.inspect.output/v1` | AW canonical | 返回 `allow`、`warn` 或 `deny` 意见 |
| `agent-sec-aw-provider-request/v1` | Provider native | 调用 Agent Sec 的三个扫描入口 |
| `agent-sec-aw-provider-response/v1` | Provider native | 返回 Agent Sec 的操作相关终态 |
| `tokenless-compression-request/v1` | Provider native | 调用 Tokenless 压缩正文 |
| `tokenless-compression-response/v1` | Provider native | 返回压缩结果、保真度和 token 估算 |
| `skill-ledger-analyze/v1` | 关联变更 | 描述 Agent Sec Skill Ledger 分析结果 |
| `cosh-ng-e2e-result` | 关联变更 | 描述 COSH-NG E2E 测试结果 |

后两份 Schema 不参与 AW Provider 发现和调用。PR 3 只修改了它们的 `$id` 域名。

### 当前建议

| Schema 组 | 是否可作为实现基线 | 冻结前重点 |
| --- | --- | --- |
| 8 份 AW canonical Schema | 可以，以 fork PoC 约束为准 | `media_type`、`retrievable`、PreTool 最终执行凭据 |
| Agent Sec request/response | 可以，以判别联合和 typed validator 为准 | 新语言与新 operation 必须版本化 |
| Tokenless request/response | 可以，以媒体类型与 source fidelity 修正为准 | frontend identity、恢复引用与 taxonomy |
| Skill Ledger / COSH E2E | 不属于 Provider 调用基线 | 保持独立，不能当作 Receipt 或 adoption 证据 |

判断一份 Schema 是否合理时，依次检查四个问题。

1. 字段由谁产生，调用方能否伪造。
2. 字段描述业务结果、执行终态还是最终环境动作。
3. 摘要覆盖哪一段准确字节，改写后由谁重新确认。
4. 正文能否进入 Receipt 或 Ledger；长期记录必须保持 content-free。

## 4. AW canonical Schema

### 4.1 Context Projection 输入

![context.projection.prepare/v1 输入](images/schemas/context-projection-prepare-input-v1.svg)

**语义职责**

| 字段 | 语义 |
| --- | --- |
| `artifact.id` | AW Core 为一份不可变输入分配的 `art_<uuid>` 身份 |
| `artifact.digest` | `artifact.content` 精确 UTF-8 字节的 SHA-256 |
| `artifact.content` | 本次允许 Provider 读取的瞬时模型可见正文 |
| `artifact.media_type` | 正文的媒体类型标签 |
| `artifact.origin` | `command_output`、`file_content` 等来源类别 |
| `artifact.tool_name` | 可安全提供时的工具名 |
| `boundary` | 能力发生在 Agent 循环中的位置 |
| `constraints.allow_text_reencoding` | 调用方是否允许结构化结果变成文本表示 |

**原 PR 风险**

Schema 对名称、ID 和长度的约束与 Rust 类型不完全一致。通用名称类型可接收展示文本，
Schema 又允许任意 1 至 128 字符串作为 artifact ID，而 Rust `ArtifactId` 已要求固定前缀
UUID。Schema 中带有 digest，但合同没有把“摘要对应哪一段编码字节”贯穿到 Receipt 与
最终采用阶段。

**PoC 基线如何解决**

`4d47593b` 将 `BoundedName` 收敛为 1 至 128 个非空格 printable ASCII 字节，并让
Schema 的 artifact ID 规则与现有 `ArtifactId` 的带前缀小写 canonical UUID 类型一致。
`6238842a` 在 Host 执行映射前校验 canonical input。`1328cf30` 要求 Receipt 保存
`input_schema` 和 `input_digest`，并校验它们与 Core 接纳的 invocation 完全相同。PoC
还把 `artifact.media_type` 映射到 Tokenless `input_media_type`，使
`allow_text_reencoding=false` 成为可以从 canonical input 一直核对到 candidate 的约束。

**仍需架构决策**

`media_type` 目前仍是通用 `BoundedName`，只能证明字符安全，不能证明符合 MIME 类型
语法或来自受控注册表。`artifact.content` 应明确为仅存在于即时调用链的 transient
数据，不进入通用 Receipt 或 Ledger。`before_model`、`pre_tool` 和 `proxy` 等 boundary
还需要逐项声明是否已经有真实调用方，避免枚举值被误解为已实现能力。

### 4.2 Context Projection 输出

![context.projection.prepare/v1 输出](images/schemas/context-projection-prepare-output-v1.svg)

**语义职责**

| 字段 | 语义 |
| --- | --- |
| `candidate.source_artifact_id` | 候选对应的原始 artifact 身份 |
| `candidate.source_digest` | 生成候选时使用的精确源正文摘要 |
| `candidate.content` | 仅沿即时路径返回；Core 校验，COSH 选择的候选正文 |
| `candidate.media_type` | 候选正文的媒体类型 |
| `candidate.content_type` | Provider 可选的更细内容分类 |
| `candidate.transform_chain` | 产生候选的有序转换标识 |
| `candidate.reversibility` | `lossless`、`retrievable` 或 `unrecoverable` |

**原 PR 风险**

AW 将 `lossless` 定义为能够恢复全部源信息，Tokenless 则可能在删除 `debug`、`trace`、
空字段或规范化表示后仍报告同名状态。manifest 直接转发这个值，导致已经丢失源信息的
结果满足 AW 的最强采用条件。`retrievable` 只有状态名，没有恢复引用、解析器和有效期。

**PoC 基线如何解决**

`414998b2` 把“任务相关信息仍在”与“精确源信息仍在”分开计算。任何未受治理的字段删除、
空值清理或表示规范化都会把 source fidelity 降为 `unrecoverable`；只有保留全部源信息的
候选才能映射为 AW `lossless`。PoC 还把 Tokenless 的 `output_media_type` 映射到
`candidate.media_type`。Core 在 `allow_text_reencoding=false` 时要求 candidate 与 source
媒体类型完全相同，并把转换链限制为最多 64 项。真实跨组件测试验证了文本 Toon 候选和
保持 `application/json` 的结构化候选两条路径。

当前 COSH effective-bytes 调用面向模型历史文本槽，调用方政策固定允许文本重编码。
禁止重编码的分支仍属于公共 canonical 合同，并由 Core 与真实 Tokenless 测试覆盖。未来
若 COSH 需要可配置的结构化槽位，再扩展它的 request 类型；这不改变 Schema 的公共语义。

**仍需架构决策**

`retrievable` 必须定义可验证的 resolver、恢复引用、访问权限、TTL 和失效后的行为，
否则 Core 无法把它当作可兑现保证。PoC 已用 `context_adoption/v1` 把 Provider output
envelope 摘要、COSH 选择和最终写入本地 Tool Result 的字节摘要关联起来。这个记录只
证明本地模型历史槽位已经提交，不声称远端模型已经收到或消费。`media_type` 与
`content_type` 仍需要专用类型或受控注册表，不能长期依赖通用名称字符串。

### 4.3 Content Inspect 输入

![security.content.inspect/v1 输入](images/schemas/security-content-inspect-input-v1.svg)

**语义职责**

| 字段 | 语义 |
| --- | --- |
| `artifact` | 被扫描正文及其 `id`、`digest`、媒体类型和来源 |
| `boundary` | 正文来自 Tool 执行前还是执行后 |
| `constraints.include_low_confidence` | 本次策略是否要求返回低置信度 finding |

`include_low_confidence` 是调用策略输入，不是扫描器健康状态。相同正文在不同策略下可能
得到不同 finding 集合，因此 Receipt 必须绑定包含该字段的完整 canonical input 摘要。

**原 PR 风险**

COSH 的通用 Hook 输入先被脱敏，AW Hook 再从脱敏 JSON 中提取内容。模型最终接收的却
可能是原始 Tool Result。扫描事实、artifact digest 和最终交付字节因此可能不属于同一
份内容，原 Receipt 也没有输入摘要可供追查。

**PoC 基线如何解决**

`4d47593b` 统一了 artifact 身份和 digest 的类型约束。`1328cf30` 把完整 canonical
input 的 Schema 与摘要加入 Receipt，并在 Host 返回前校验 invocation、transient output
和 Receipt 的对应关系。这可以证明 Provider 实际接收了哪份输入。

**仍需架构决策**

Receipt 本身不能证明这份输入就是 COSH 最终使用的字节。PoC 因此把 AW effective-bytes
边界放在所有通用 PostToolUse 改写完成之后，并在写入本地 Tool Result 后用
`context_adoption/v1` 再次核对摘要。PreToolUse 仍需补同等级的 executed-bytes 绑定，避免
扫描后的命令再被其他路径改写。`include_low_confidence` 的默认值和配置权也需要归属于
明确的系统策略，不能由普通 Tool 输出控制。

### 4.4 Content Inspect 输出

![security.content.inspect/v1 输出](images/schemas/security-content-inspect-output-v1.svg)

**语义职责**

| 字段 | 语义 |
| --- | --- |
| `inspection.verdict` | 完整扫描的 `clean`，或已验证前缀中的 `suspicious` / `sensitive` 事实 |
| `inspection.findings[]` | 规则 ID、类别、严重性、置信度和命中计数 |
| `inspection.scanned_bytes` | Provider 声明已经扫描的 UTF-8 字节数 |
| `inspection.truncated` | 是否没有覆盖完整 artifact |

Finding 不包含命中原文、偏移或上下文。这是 content-free 安全结果的正确方向。

**原 PR 风险**

Schema 允许 `clean` 同时携带 finding，也允许 `clean + truncated=true`，还允许
`suspicious` 或 `sensitive` 没有任何 finding。`count=0` 也能通过，业务状态之间可以
互相矛盾。

**PoC 基线如何解决**

`4d47593b` 同时在 JSON Schema 和 Rust typed validation 中加入状态表。`clean` 必须
没有 finding 且 `truncated=false`；发现类 verdict 至少有一个 finding；每项 `count`
从 1 开始，数组数量有固定上限。发生截断时，`scanned_bytes` 必须是实际完成解码并扫描的
UTF-8 前缀长度，满足 `0 < scanned_bytes < input_bytes`；未截断时必须等于完整输入长度。
部分扫描已经发现风险时，可以返回 `suspicious` 或 `sensitive` 并保留
`truncated=true`。没有 finding 的截断扫描不得返回 `clean`，扫描异常则进入 `error` 和
observation gap。

**仍需架构决策**

canonical verdict 保持 `clean | suspicious | sensitive` 的闭集，不新增
`indeterminate`。没有 finding 的截断、扫描失败或无可用实现没有安全结论，应通过
Provider `failed/uncertain`、`ObservationGapReason` 或门禁 degradation 表达。已经发现
风险的部分扫描可以保留业务 verdict 与 `truncated=true`。PoC 已修正多字节 UTF-8 截断
落在字符中间时的计数，只记录实际可解码前缀，不把未扫描的半个字符计入
`scanned_bytes`。未来新增部分覆盖扫描器时，需要沿用同一 coverage 规则。

### 4.5 Code Inspect 输入

![security.code.inspect/v1 输入](images/schemas/security-code-inspect-input-v1.svg)

**语义职责**

| 字段 | 语义 |
| --- | --- |
| `artifact` | 被检查代码及其 source identity |
| `boundary` | 代码出现的 Agent 边界 |
| `constraints.language` | `auto`、`bash` 或 `python` 扫描策略 |

**原 PR 风险**

Core 固定发送 `auto`，Agent Sec 却把除显式 `python` 外的所有输入都交给 Bash 扫描器。
普通 Python 内容可能只经过 Bash 规则后得到 `clean`，与 `auto` 给读者的覆盖承诺不符。

**PoC 基线如何解决**

`d1d813d5` 将 `auto` 定义为对同一份正文分别运行 Bash 与 Python 规则。两次扫描都成功
才返回 completed，`language_detected` 为 `mixed`；任一引擎失败都返回 closed `error`
终态，不产生虚假的 `clean`。纯 Bash、纯 Python、mixed 和引擎失败均有协议测试。

**仍需架构决策**

当前 `auto` 的“双扫描”定义适合只支持 Bash 和 Python 的 v1。未来增加语言时应通过
Capability 或协议版本明确扩展规则集合，不能让同一个 `auto` 在不同 Provider 上静默
代表不同覆盖面。调用预算也需要覆盖多引擎扫描的总成本。

### 4.6 Code Inspect 输出

![security.code.inspect/v1 输出](images/schemas/security-code-inspect-output-v1.svg)

**语义职责**

| 字段 | 语义 |
| --- | --- |
| `inspection.verdict` | 对已完成规则集合的安全结论 |
| `inspection.findings[]` | content-free 规则计数 |
| `inspection.scanned_bytes` | 被检查正文的 UTF-8 大小 |
| `inspection.truncated` | 是否只分析了部分输入 |
| `inspection.language_detected` | 实际完成的 `bash`、`python`、`mixed` 或 `unknown` |

**原 PR 风险**

`language_detected` 可以缺失，`unknown` 或 `truncated=true` 仍能和 `clean` 组合。代码
执行风险还被放入偏向敏感内容的类别，无法区分 secret 与 dangerous construct。

**PoC 基线如何解决**

`4d47593b` 要求 `language_detected` 必填，加入 `mixed`，并把危险代码映射到
`dangerous_pattern`。与 Content Inspect 相同的状态校验禁止 `clean + truncated` 和
无证据的发现 verdict。`d1d813d5` 的 Agent Sec completed 分支只返回 `bash`、`python`
或 `mixed`；无法完成分类或扫描时走 error，再由 Core 记录 gap。Core 同时核对
`scanned_bytes` 与输入 UTF-8 字节数，不能只信任 Provider 声明。

**仍需架构决策**

canonical `unknown` 可以保留给其他 Provider，但必须定义它只代表“已完整扫描但语言分类
未知”，还是代表“覆盖无法确认”。后一种含义不能与 `clean` 并存，应转为 observation
gap。该语义需要 conformance vector 固定，不能只靠字段名称推断。

### 4.7 Command Inspect 输入

![security.command.inspect/v1 输入](images/schemas/security-command-inspect-input-v1.svg)

**语义职责**

| 字段 | 语义 |
| --- | --- |
| `command.content` | 即将执行的完整命令字节 |
| `command.digest` | 命令 UTF-8 字节的 SHA-256 |
| `command.language` | 命令的扫描策略 |
| `command.tool_name` | 执行该命令的工具名 |
| `boundary` | 固定为 `pre_tool` |

**原 PR 风险**

Schema 能证明请求内同时存在 command 和 digest，却不能证明扫描后没有其他 Hook 修改
命令，也不能证明系统实际执行的命令仍与该 digest 相同。脱敏后的 Hook 输入还可能使
Provider 检查的只是替代文本。

**PoC 基线如何解决**

`4d47593b` 统一 tool name 和 digest 的词法约束。`6238842a` 校验 canonical 与 native
请求形状，`1328cf30` 让 Receipt 绑定完整输入摘要。`d1d813d5` 让 command `auto` 同时
运行 Bash 和 Python 规则，并在任一扫描失败时返回 error。

**仍需架构决策**

COSH 必须在所有 PreToolUse patch 聚合完成后计算并检查
`digest(scanned_command) == digest(executed_command)`。若检查后仍允许改写命令，应重新
调用门禁或拒绝执行。只有 Receipt 输入摘要还不足以证明最终 side effect 使用了同一命令。

### 4.8 Command Inspect 输出

![security.command.inspect/v1 输出](images/schemas/security-command-inspect-output-v1.svg)

**语义职责**

| 字段 | 语义 |
| --- | --- |
| `decision.verdict` | Provider 对待执行命令的 `allow`、`warn` 或 `deny` 意见 |
| `decision.reasons[]` | 可稳定关联的规则理由代码 |
| `decision.findings[]` | 支撑门禁意见的 content-free 规则计数 |
| `decision.scanned_bytes` | 被扫描命令的 UTF-8 字节数 |

**原 PR 风险**

Schema 允许 `allow` 携带 critical finding，也允许 `warn` 或 `deny` 没有理由和 finding。
canonical 输出没有 coverage 或实际语言字段，调用方可能把不完整扫描误读为允许。

**PoC 基线如何解决**

`4d47593b` 要求 `allow` 不得携带 reasons 或 findings，`warn/deny` 必须同时有理由和
finding，并限制数量与计数。`d1d813d5` 的 typed native model 还要求 reason 必须引用
已返回 finding，`findings_total` 必须等于各 finding count 之和。扫描失败不会产生
`allow`，而是由 Core 映射为 `NotMediated` 加明确 degradation。

**仍需架构决策**

Agent Sec native response 已有 `language_detected`，canonical command output 尚未携带该
字段。需要决定门禁审计是否必须知道实际规则集。如果未来存在截断式命令扫描，还需要
在 v2 增加 coverage 合同；v1 只能在 Provider 保证全量扫描或失败的前提下成立。

## 5. Provider native Schema

### 5.1 Agent Sec native request

![Agent Sec native request/v1](images/schemas/agent-sec-aw-provider-request-v1.svg)

**语义职责**

| 分支或字段 | 语义 |
| --- | --- |
| `operation=content_inspect` | 要求 `content`、`source` 和 `include_low_confidence` |
| `operation=code_inspect` | 要求 `content` 和 `language` |
| `operation=command_inspect` | 要求 `content` 和 `language` |
| `protocol_version=1` | 固定本地进程协议版本 |

**原 PR 风险**

三个 operation 共用一个可选字段袋。`command_inspect` 缺少 language、content scan 携带
代码字段等不合法组合都能通过，Provider 只能在运行时猜测请求意图。

**PoC 基线如何解决**

`d1d813d5` 将请求改为以 `operation` 为 discriminator 的三个 `oneOf` 分支，并在 JSON
Schema 与 Pydantic model 中禁止额外字段。manifest 为每项 Capability 写入固定 operation，
Host 在执行前校验映射后的 native request。

**仍需架构决策**

新增 operation 或语言会改变 native 协议能力，应通过明确版本演进。正文大小目前由 Host
manifest limit 约束，而不是在 Schema 中重复一份上限；该责任划分需要保留一致。

### 5.2 Agent Sec native response

![Agent Sec native response/v1](images/schemas/agent-sec-aw-provider-response-v1.svg)

**语义职责**

| 分支或字段 | 语义 |
| --- | --- |
| `content/code/command + completed` | 对应 operation 的完整业务结果 |
| `skipped` | 请求适用性不足，没有安全结论 |
| `error` | 扫描器失败，没有安全结论 |
| `operation` | 回显本次响应所对应的请求操作 |
| `engine` | completed 分支使用的闭集实现标识 |
| `findings_total`、`scanned_bytes` | 计量和覆盖事实 |
| `skip_reason`、`error_code` | skipped 或 error 分支的闭集原因代码 |

**原 PR 风险**

原响应使用多个 operation 的联合字段，completed 可以缺少 verdict，content scan 也可能
返回 command verdict。响应不回显 operation，Host 无法证明 stdout 对应本次请求。

**PoC 基线如何解决**

`d1d813d5` 建立 operation 与 disposition 双重判别的 completed、skipped 和 error 分支，
失败字段使用闭集代码且不携带正文。manifest 声明 request/response 的 operation correlation，
`6238842a` 后的 Host 在映射前校验 native response，并拒绝缺失或不匹配的 correlation。

**仍需架构决策**

`findings_total == sum(findings[].count)` 由 Pydantic typed validation 保证，普通 JSON
Schema 不容易完整表达这一求和不变量。需要保留跨语言 conformance vectors，防止未来
非 Python 实现只满足字段形状而破坏计量语义。

### 5.3 Tokenless CompressionRequest

![Tokenless CompressionRequest/v1](images/schemas/tokenless-compression-request-v1.svg)

**语义职责**

| 字段 | 语义 |
| --- | --- |
| `protocol_version` | 固定为 1 |
| `content` | 允许压缩的模型可见正文 |
| `input_media_type` | source 正文的媒体类型；PoC Provider 路径必须提供 |
| `agent_id` | Tokenless 定义的稳定 frontend 身份 |
| `session_id`、`tool_use_id`、`tool_name` | 可选调用归属 |
| `seam`、`content_origin` | Agent 边界和内容来源 |
| `capabilities` | Host 是否能替换、发布恢复工具和接收文本重编码 |

**原 PR 风险**

顶层和 `capabilities` 允许任意额外字段，多个身份字符串没有有效约束。manifest 把 AW
`environment_id` 直接映射到 Tokenless `agent_id`，但前者是一份 Environment instance
身份，后者原义是 `claude-code` 一类稳定 frontend 名称。原 native request 也没有
`input_media_type`。当 canonical input 声明 `allow_text_reencoding=false` 时，Tokenless
无法知道结构化槽位原本是 `application/json`，这个约束没有贯穿到组件协议。

**PoC 基线如何解决**

`414998b2` 关闭顶层和 `capabilities` 的额外字段，并要求可选身份非空。`6238842a` 让
Host 对映射后的 native request 执行 Schema 校验，避免 manifest 或调用方产生协议外
形状。PoC manifest 进一步把 `agent_id` 固定为已准入常量 `aw-provider`，不再复用
environment instance ID，并把 canonical `artifact.media_type` 映射到 native
`input_media_type`。`replace_with_text` 继续映射 `allow_text_reencoding`，两项合起来表达
“可以改成文本”或“必须保留结构化媒体类型”。

**仍需架构决策**

常量解决了当前语义错位，但没有提供真实 frontend 归因。如果这个维度以后参与统计或
策略，AW canonical contract 应增加独立、版本化的 frontend identity；不能重新复用
`environment_id`。这些 native 字符串还需要与 Tokenless typed model 共享明确的字节上限。

### 5.4 Tokenless CompressionResponse

![Tokenless CompressionResponse/v1](images/schemas/tokenless-compression-response-v1.svg)

**语义职责**

| 字段 | 语义 |
| --- | --- |
| `disposition` | 是否应用、试算、原样返回、超时或失败 |
| `output` | `applied` 时为候选，其余状态为原始正文 |
| `output_media_type` | `output` 的事实媒体类型；PoC writer 总是显式返回 |
| `compressor_chain` | 实际产生候选的转换链 |
| `reversibility`、`stash_keys` | 精确源信息能否本地恢复或通过治理状态恢复 |
| `before_tokens`、`after_tokens`、`tokenizer_id` | 同一计数方法下的 token 估算 |
| `diagnostic` | 仅 error 时允许的有界诊断 |

**原 PR 风险**

Schema 没有约束 disposition、output、转换链、stash 和 diagnostic 的组合。更严重的是，
Tokenless 会把“没有删除任务相关信息”报告为 `lossless`，与 AW 的精确源信息定义冲突。
原响应没有 `output_media_type`，manifest 无论实际表示是什么都把 candidate 标成
`text/plain`，Core 也没有执行 `allow_text_reencoding` 约束。

**PoC 基线如何解决**

`414998b2` 在 JSON Schema 与 Rust `CompressionResponse::validate` 中加入状态表。只有
`applied/dry_run` 可以有转换链；未应用状态必须原样输出并报告相同 token 数；只有 error
可带 diagnostic；`retrievable` 必须有 stash key，`lossless` 不得有 stash key。压缩器
另行计算 exact source fidelity，清理、删除或未追踪规范化一律降为 `unrecoverable`。PoC
新增并校验 `output_media_type`。Toon 结果为 `text/plain`；保持结构化槽位时继承
`application/json`，Host 再把该值映射到 canonical candidate。

**仍需架构决策**

canonical candidate 目前没有承载 `stash_keys` 或通用 evidence reference，因此
`retrievable` 不能成为最终采用依据。Tokenless 的 `content_type`、compressor ID 和
tokenizer ID 也应进入受控 taxonomy；它们可以留在 transient outcome，但不能作为任意
Provider 文本直接进入长期 Ledger。

## 6. 关联但不参与 Provider 的 Schema

### 6.1 Skill Ledger analyze result

![Skill Ledger analyze result/v1](images/schemas/skill-ledger-analyze-v1.svg)

**语义职责**

该 Schema 描述 Agent Sec Skill Ledger 的离线分析结果。它不经过 Provider Host，也不是
AW Ledger record。

| 字段 | 语义 |
| --- | --- |
| `schema_version`、`engine_version` | 报告形状与分析引擎版本 |
| `status` | `pass`、`warn`、`deny` 或 `error` 总结 |
| `coverage_complete` | 声明扫描器集合是否完整完成 |
| `scanners[]` | 各扫描器的 finding 和 metadata |
| `errors[]` | 分析失败列表 |

**原 PR 风险**

PR 3 只修改 `$id` 域名。`metadata` 是无约束对象，如果未来直接复用到 AW Receipt 或
Ledger，会破坏 content-free 边界。

**PoC 基线如何解决**

PoC 没有改动这份 Schema，因为它不属于 AW Provider 调用合同。AW Ledger 在
`5d7f6b62` 中改用独立 typed body、闭集字段和规则 ID 摘要，没有复用该 metadata。

**仍需架构决策**

两类 Ledger 应保持名称与存储合同隔离。若未来需要引用 Skill Ledger，只保存受治理的
evidence reference 和 digest，不复制其任意 metadata。

### 6.2 COSH-NG E2E result

![COSH-NG E2E result](images/schemas/cosh-ng-e2e-result.svg)

**语义职责**

该 Schema 记录一次 E2E 测试报告，不参与线上 Provider 调用。

| 字段 | 语义 |
| --- | --- |
| `schema_version`、`run_id`、`profile` | 报告版本、运行身份和环境档位 |
| `started_at`、`finished_at` | 测试时间窗口 |
| `artifact` | 被测产物路径、SHA-256 与 Git 提交 |
| `environment` | 测试环境事实 |
| `cases[]` | 每个 case 的状态、耗时、指标和产物 |
| `cleanup` | 临时资源清理状态 |

**原 PR 风险**

PR 3 同样只修改 `$id`。`environment` 和 `metrics` 是无约束对象，时间先后和 cleanup
状态需要由测试生成器校验。

**PoC 基线如何解决**

PoC 没有把它改造成 AW 合同。真实 Provider E2E 应输出独立报告，并把 Schema、类型和
manifest conformance 测试结果作为验证证据。

**仍需架构决策**

如果该报告进入共享长期存储，需要另行定义可持久化环境字段和敏感数据规则。它不应被
当作 ProviderReceipt 或 final adoption 证据。

## 7. 跨 Schema 语义状态

| 主题 | 原 PR | PoC 基线 | 接口冻结前仍需决定 |
| --- | --- | --- | --- |
| 稳定名称与 ID | Schema 与类型词法不一致 | printable ASCII `BoundedName`；类型化 UUID ID | 为 `media_type` 等领域值建立专用类型 |
| 安全 verdict | 矛盾状态可通过 | Schema 与 typed validation 共用状态表 | verdict 保持闭集；失败统一进入 gap/degradation |
| `auto` 语言 | 实际只走 Bash | Bash 与 Python 都成功才产生 `mixed` 结果 | 新语言通过版本化扩展 |
| Schema 权威性 | 只校验文件和摘要 | 校验 canonical input、native request、native response、canonical output | 维护跨语言 conformance vectors |
| Tokenless `lossless` | 任务相关语义被当作源信息保真 | 删除、清理、规范化后为 `unrecoverable` | 定义 `retrievable` resolver、reference 和 TTL |
| Tokenless 媒体类型 | native 协议不传 source/output 类型，candidate 固定成文本 | `input_media_type -> output_media_type -> candidate.media_type`，Core 执行重编码约束 | 建立 MIME 类型或准入注册表 |
| Receipt 来源 | 没有输入 Schema 和摘要 | 输入、输出、manifest 与 invocation 互相校验 | 为更多能力定义同等级来源绑定 |
| Ledger content freedom | Provider 文本可进入长期记录 | rule/reason 哈希；只保存转换数和闭集元数据 | 建立媒体类型与 taxonomy 注册表 |
| COSH 最终采用 | Hook replacement 被当作结果 | `context_adoption/v1` 绑定 plan、candidate envelope、选择与本地历史字节 | 远端模型消费证据不在当前保证内 |
| Checkpoint | 没有可信 State Provider 合同 | 使用独立 Guarded V2 与 Gateway 状态边界验证 | 暂不创建 AW Schema，先冻结 generation、UID、evidence 语义 |

安全 verdict 不增加 `indeterminate`。这是有意的职责分离。`clean` 只表示完整扫描没有
finding；`suspicious` 或 `sensitive` 可以携带 `truncated=true`，表示部分覆盖已经发现
风险。`scanned_bytes` 是实际扫描的 UTF-8 前缀长度，不是配置上限。没有可用风险事实的
截断、Provider error、timeout、invalid output 和无实现进入 `ObservationGapReason`、
`GateDegradation` 或 Provider 终态，避免一个枚举同时表达“发现了什么”和“调用是否成功”。

Checkpoint 也不应为了形式统一而提前增加 AW Provider Schema。当前 Host 的 one-shot
`exec-json/v1` 不能表达 workspace generation、peer UID、Unix socket instance 和 effect
reconciliation。只有 State Provider 生命周期与证据合同冻结后，才适合新增 canonical
Checkpoint Capability。

## 8. JSON Schema 之外的运行包络

上面的 14 份 Schema 定义 Capability payload。一次调用还需要运行包络把 payload 与身份、
终态和最终采用关联起来。这些包络目前由 Rust typed contract 定义，不应误算为额外的
Provider JSON Schema 文件。

### ProviderInvocationResult

| 部分 | 关键字段 | 作用 |
| --- | --- | --- |
| `outcome.output` | canonical output schema、digest、body | 瞬时返回 candidate 或安全事实，可以含正文 |
| `receipt` 输入身份 | capability、input schema、input digest、scope | 证明 Provider 处理的是哪次调用的哪份输入 |
| `receipt` 输出身份 | disposition、output schema、output digest、output bytes | 证明瞬时 output 与无正文事实相符 |
| `receipt` 来源 | provider/version、manifest digest、binding/generation | 证明使用了哪个已准入实现 |
| `receipt` 其他事实 | meters、evidence refs、时间、受限 error | 提供 content-free 计量和终态 |

Host 必须把 `outcome` 和 `receipt` 一起返回 AW Core。只返回 receipt 会丢掉 Tokenless 的
实际压缩结果，只返回 output 又无法验证结果来源。

### PostToolUse plan 与 final adoption

| Typed body | 关键字段 | 证明范围 |
| --- | --- | --- |
| `post_tool_use_plan/v1` | source identity、observations、gaps、projection、invocation refs | 每个计划步骤产生了事实还是明确缺口 |
| `context_adoption/v1` | plan event、source/candidate/effective digest、字节数、decision、reason | COSH 最终写入本地历史的是 candidate 还是 source |
| `pre_tool_use_gate/v1` | command digest、字节数、gate、reason digests、degradation、invocation | Core 产生了哪项门禁要求 |

PoC validator 要求 plan 中每个 Observe capability 都被 observation 或 gap 覆盖；同一
capability 可以 fan-out 到多个 Provider，但 `(capability, provider_id)` 和 invocation ID
不得重复。Gap 的 reason、provider、receipt disposition 与 error code 必须互相匹配。
Invocation reference 的 `attempt_id`、`tool_use_id` 必须与 Ledger header scope 一致。
Adoption 再要求引用相同 plan 和相同 invocation 集合。Projection 的
`transform_count` 不得超过 64；没有 candidate 时必须为 0。
当前 validator 会拒绝 `LedgerUnavailable` gap，因为尚未存在可信、类型封闭的 Ledger
不可用证明协议；调用方不能用这个 reason 绕过 observation completeness。

这组约束解决“记录存在但无法证明属于同一 Tool Call”的问题。它仍只证明 COSH 的本地
history mutation，不证明远端模型已接收或消费该内容。PreTool gate 也尚未证明命令在扫描
后没有再次被改写，因此还需要最终 executed-command digest credential。

## 9. Schema 变更的最小开发闭环

修改一份 Provider Schema 时，需要同时检查以下对象。

1. AW canonical Rust 类型是否表达同一字段和闭集枚举。
2. canonical Schema 及 Provider 包副本是否逐字节一致。
3. `provider.toml` 中 Schema digest 和 mapping 是否同步。
4. 组件 native Schema 与 Python 或 Rust typed model 是否一致。
5. Host 是否校验 canonical input、native request、native response 和 canonical output。
6. Core 是否校验 digest、状态组合、source identity 和 Receipt invocation identity。
7. Ledger 是否只保存闭集元数据、摘要与引用，不复制正文或 Provider 任意文本。
8. COSH 是否证明最终交付或执行的字节与被检查、被采用的 digest 相同。

推荐用同一组 conformance vectors 同时驱动 Schema、typed model、manifest mapping 和真实
Provider 进程测试。Schema 文件通过校验只代表“形状正确”，完整闭环还必须证明语义状态、
调用来源和最终字节一致。

## 10. 物理文件与图册维护

| 文件类别 | 数量 | 说明 |
| --- | --- | --- |
| `aw-contracts` canonical | 8 | AW 权威定义 |
| Provider 包 canonical 副本 | 8 | 与权威文件逐字节一致 |
| Provider native | 4 | Tokenless 和 Agent Sec 各一对 |
| 关联变更 | 2 | 只修改 `$id`，不参与 Provider |

<details>
<summary>展开 22 个物理文件路径</summary>

AW canonical 权威文件

1. `src/aw/crates/aw-contracts/schemas/context-projection-prepare-input-v1.schema.json`
2. `src/aw/crates/aw-contracts/schemas/context-projection-prepare-output-v1.schema.json`
3. `src/aw/crates/aw-contracts/schemas/security-content-inspect-input-v1.schema.json`
4. `src/aw/crates/aw-contracts/schemas/security-content-inspect-output-v1.schema.json`
5. `src/aw/crates/aw-contracts/schemas/security-code-inspect-input-v1.schema.json`
6. `src/aw/crates/aw-contracts/schemas/security-code-inspect-output-v1.schema.json`
7. `src/aw/crates/aw-contracts/schemas/security-command-inspect-input-v1.schema.json`
8. `src/aw/crates/aw-contracts/schemas/security-command-inspect-output-v1.schema.json`

Provider 包中的 canonical 副本

1. `providers/tokenless/schemas/context-projection-prepare-input-v1.schema.json`
2. `providers/tokenless/schemas/context-projection-prepare-output-v1.schema.json`
3. `providers/agent-sec-core/schemas/security-content-inspect-input-v1.schema.json`
4. `providers/agent-sec-core/schemas/security-content-inspect-output-v1.schema.json`
5. `providers/agent-sec-core/schemas/security-code-inspect-input-v1.schema.json`
6. `providers/agent-sec-core/schemas/security-code-inspect-output-v1.schema.json`
7. `providers/agent-sec-core/schemas/security-command-inspect-input-v1.schema.json`
8. `providers/agent-sec-core/schemas/security-command-inspect-output-v1.schema.json`

Provider native 文件

1. `providers/tokenless/schemas/tokenless-compression-request-v1.schema.json`
2. `providers/tokenless/schemas/tokenless-compression-response-v1.schema.json`
3. `providers/agent-sec-core/schemas/agent-sec-aw-provider-request-v1.schema.json`
4. `providers/agent-sec-core/schemas/agent-sec-aw-provider-response-v1.schema.json`

关联变更文件

1. `src/agent-sec-core/agent-sec-cli/src/agent_sec_cli/skill_ledger/analyze.schema.json`
2. `src/cosh-ng/e2e/result.schema.json`

</details>

仓库另有未被 PR 3 修改的 `website/agent-index/schema.json`，不计入上述 22 个文件。

14 张 SVG 由 `diagram-sources/schema-catalog.json` 和
`diagram-sources/render-schema-diagrams.mjs` 确定性生成。修改 Schema 后应重新生成图册，
并检查 canonical 副本、manifest digest、typed model 与 source file 是否仍然一致。
