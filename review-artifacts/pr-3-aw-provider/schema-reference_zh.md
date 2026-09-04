# PR 3 Schema 图册与语义评审

本文档面向第一次接触 AW Provider 的研发、测试和架构评审人员。阅读者只需了解
JSON 对象、字符串、数组和进程的基本概念。文档说明本 PR 中每份 Schema 的字段、
职责、合理之处及需要在接口冻结前解决的矛盾。

审查基线为 `8574ecb022ec9ffc68e1a71e30f2186b6ec81674`，PR 头提交为
`42d07649409ecd5bb023056b28545efbd9325ef2`。

## 1. 结论摘要

本 PR 涉及 22 个物理 JSON Schema 文件。8 份 AW canonical Schema 同时复制到
Provider 包中，因此按内容和语义去重后共有 14 份逻辑 Schema。

整体分层是合理的。AW 用 canonical Schema 固定跨组件语义，Tokenless 和 Agent Sec
用 native Schema 保留组件自己的协议，`provider.toml` 负责声明二者之间的字段映射。
这种设计避免在 Provider Host 中编写 `if provider == tokenless` 一类专用分支。

当前不适合直接冻结 v1 的原因主要有四类。

1. Schema 与 Rust 类型的长度和标识符约束不一致。
2. 输出中的 verdict、finding、coverage 和 disposition 缺少状态一致性约束。
3. Host 校验 Schema 文件及摘要，但不会用 Schema 校验实际请求和响应实例。
4. Tokenless 与 AW 对 `lossless` 的定义不同，会把实际删除字段的结果当成完全可逆。

因此，Schema 可以继续作为改造中心，但必须同时把 Rust 类型、manifest mapping、
真实 Provider 响应和 conformance tests 纳入同一次合同冻结。

## 2. JSON Schema 基础

JSON Schema 是对 JSON 文档形状和取值范围的机器可读约束。以下关键字足以阅读本图册。

| 关键字 | 含义 |
| --- | --- |
| `type: object` | 当前值必须是 JSON 对象 |
| `properties` | 对象允许出现的字段及各字段类型 |
| `required` | 必须出现的字段集合 |
| `additionalProperties: false` | 拒绝未声明字段 |
| `enum` | 字段只能取给定值之一 |
| `const` | 字段只能取一个固定值 |
| `pattern` | 字符串必须满足正则表达式 |
| `maxLength` | JSON Schema 中按 Unicode 字符计数的最大长度 |
| `maxItems` | 数组元素数量上限 |
| `minimum` / `maximum` | 数值上下限 |

Schema 能验证“形状”，但不能自动证明所有业务事实。例如，它不能仅凭一个 `digest`
字段证明该值确实等于 `content` 的 SHA-256，也不能证明传入的命令正是系统随后执行的
命令。这些跨字段和跨系统不变量仍需由代码与测试保证。

## 3. 三类 Schema 与数据方向

| 类型 | 维护方 | 作用 | 是否跨 Provider 稳定 |
| --- | --- | --- | --- |
| AW canonical | AW 团队 | 定义 Capability 的公共业务语义 | 是 |
| Provider native | 组件团队 | 定义组件可执行程序的 stdin/stdout 协议 | 否 |
| 关联测试/分析 | 对应组件 | 描述测试结果或独立分析结果 | 不参与 Provider 路由 |

一次调用的主要数据方向如下。

```text
COSH 私有事件
  → AW Hook 生成 canonical input
  → Provider Host 按 provider.toml 映射为 native request
  → 组件返回 native response
  → Provider Host 映射为 canonical output 与 receipt
  → AW Core 校验
  → COSH 决定是否真正采用
```

## 4. 22 个物理文件清单

### 4.1 AW canonical 权威文件

1. `src/aw/crates/aw-contracts/schemas/context-projection-prepare-input-v1.schema.json`
2. `src/aw/crates/aw-contracts/schemas/context-projection-prepare-output-v1.schema.json`
3. `src/aw/crates/aw-contracts/schemas/security-content-inspect-input-v1.schema.json`
4. `src/aw/crates/aw-contracts/schemas/security-content-inspect-output-v1.schema.json`
5. `src/aw/crates/aw-contracts/schemas/security-code-inspect-input-v1.schema.json`
6. `src/aw/crates/aw-contracts/schemas/security-code-inspect-output-v1.schema.json`
7. `src/aw/crates/aw-contracts/schemas/security-command-inspect-input-v1.schema.json`
8. `src/aw/crates/aw-contracts/schemas/security-command-inspect-output-v1.schema.json`

### 4.2 Provider 包中的 canonical 副本

以下文件与对应权威文件逐字节一致，供 Provider 包独立安装和摘要校验。

1. `providers/tokenless/schemas/context-projection-prepare-input-v1.schema.json`
2. `providers/tokenless/schemas/context-projection-prepare-output-v1.schema.json`
3. `providers/agent-sec-core/schemas/security-content-inspect-input-v1.schema.json`
4. `providers/agent-sec-core/schemas/security-content-inspect-output-v1.schema.json`
5. `providers/agent-sec-core/schemas/security-code-inspect-input-v1.schema.json`
6. `providers/agent-sec-core/schemas/security-code-inspect-output-v1.schema.json`
7. `providers/agent-sec-core/schemas/security-command-inspect-input-v1.schema.json`
8. `providers/agent-sec-core/schemas/security-command-inspect-output-v1.schema.json`

### 4.3 Provider native 文件

1. `providers/tokenless/schemas/tokenless-compression-request-v1.schema.json`
2. `providers/tokenless/schemas/tokenless-compression-response-v1.schema.json`
3. `providers/agent-sec-core/schemas/agent-sec-aw-provider-request-v1.schema.json`
4. `providers/agent-sec-core/schemas/agent-sec-aw-provider-response-v1.schema.json`

### 4.4 关联变更文件

1. `src/agent-sec-core/agent-sec-cli/src/agent_sec_cli/skill_ledger/analyze.schema.json`
2. `src/cosh-ng/e2e/result.schema.json`

后两份文件在本 PR 中只修改 `$id` 域名，不是 AW Ledger Schema，也不参与 Provider
发现、路由或调用。仓库中另有未被本 PR 修改的 `website/agent-index/schema.json`，不计入
上述 22 个文件。

## 5. AW canonical Schema

### 5.1 Context Projection 输入

![context.projection.prepare/v1 输入](images/schemas/context-projection-prepare-input-v1.svg)

该输入把一份即将进入模型上下文的 Tool Result 表示为不可变 artifact。`id` 用于身份
关联，`digest` 用于绑定正文，`content` 是 Provider 实际处理的字符串，`boundary`
说明调用发生在哪个 Agent 边界，`constraints` 则声明是否允许把结构化内容改写成文本。

结构分层清楚，适合作为 Provider 无关的公共输入。冻结前需统一 `tool_name` 的字符数与
Rust 侧 128 UTF-8 bytes 限制，并明确 digest 的计算字节域。当前 Core 只走
`post_tool`，其他 boundary 应标为保留值或补齐实现状态。

### 5.2 Context Projection 输出

![context.projection.prepare/v1 输出](images/schemas/context-projection-prepare-output-v1.svg)

`candidate` 是建议值，不表示已经进入模型。`source_artifact_id` 与 `source_digest`
用于防止旧候选或错源候选被采用；`transform_chain` 记录转换顺序；`reversibility`
声明可恢复性。

该设计正确保留了 Environment 的最终采纳权。主要阻断项是 `lossless` 语义：AW 定义为
保留全部源信息，而 Tokenless 删除 `debug`、`trace`、空字段后仍可能返回
`lossless`。不能通过削弱 AW 的定义来兼容，应由 Tokenless 的 AW 适配层给出更严格的
可恢复性结果。`retrievable` 还需要 recovery reference、有效期和 resolver 合同。

### 5.3 Content Inspect 输入

![security.content.inspect/v1 输入](images/schemas/security-content-inspect-input-v1.svg)

该输入复用 artifact 语义，将模型可见正文提交给凭据和敏感信息扫描。能力属于
Observe，只报告事实；`include_low_confidence` 由调用方决定是否保留低置信度命中。

最大矛盾不在字段树，而在调用字节域。当前 COSH 会先脱敏 HookInput，AW 扫描的是脱敏
副本，而后续模型仍可能看到原始 Tool Result。Schema 和 digest 必须明确绑定最终入模的
同一份字节，否则安全结论和审计摘要都可能指向错误内容。

### 5.4 Content Inspect 输出

![security.content.inspect/v1 输出](images/schemas/security-content-inspect-output-v1.svg)

输出只返回规则、类别、严重性、置信度和计数，不返回命中原文或偏移。这一选择降低了
日志与 Ledger 二次泄露的风险。

Schema 当前允许 `verdict=clean` 同时携带 critical finding，也允许
`truncated=true` 后仍给 clean。建议增加条件约束或在 Rust typed validation 中统一
拒绝。`count` 应从最小值 1 开始，coverage 不完整时应使用 `indeterminate` 或等价状态。

### 5.5 Code Inspect 输入

![security.code.inspect/v1 输入](images/schemas/security-code-inspect-input-v1.svg)

该输入在 artifact 之外增加 `language`，可取 `auto`、`bash` 或 `python`。将语言假设
写入合同是合理的，因为同一文本在不同解析器下会得到完全不同的风险结论。

当前 Agent Sec 对 `auto` 没有自动识别逻辑，除显式 `python` 外均按 Bash 扫描；Core
又固定发送 `auto`。普通 Python 中的 `pickle.loads` 等规则因此可能漏检并返回 clean。
v1 应删除 `auto` 并要求上游明确语言，或定义可靠检测、双扫描及 unknown 降级规则。

### 5.6 Code Inspect 输出

![security.code.inspect/v1 输出](images/schemas/security-code-inspect-output-v1.svg)

该输出增加 `language_detected`，有助于调用方判断结论适用范围。finding 结构与 Content
Inspect 保持一致，便于 Core 使用统一观察结果类型。

`language_detected=unknown`、`truncated=true` 和 `verdict=clean` 仍可同时出现，语义不
可靠。危险代码被映射为 `sensitive` 也混合了“内容敏感”和“执行危险”两个概念。
建议将 inspection verdict 设计为 `clean / finding / indeterminate`，风险类别继续放在
finding 中。

### 5.7 Command Inspect 输入

![security.command.inspect/v1 输入](images/schemas/security-command-inspect-input-v1.svg)

该 Capability 属于 Mediate，发生在 `pre_tool`。`command.content` 表示即将执行的命令，
`digest` 用于审计绑定，`language` 决定扫描器，`tool_name` 提供工具上下文。

将 Mediate 与 Observe 分离是正确的权力设计。不过 Schema 只能声明 content，不能证明
COSH 后续执行的是同一命令。Environment 必须在聚合完成后验证 digest，并记录最终执行
决定。`auto` 与 `tool_name` 限制仍有和 Code Inspect 相同的问题。

### 5.8 Command Inspect 输出

![security.command.inspect/v1 输出](images/schemas/security-command-inspect-output-v1.svg)

`verdict` 是 `allow / warn / deny`，`reasons` 是稳定理由码，`findings` 提供事实依据。
封闭枚举使 Core 可以确定地映射为 Allow、Ask 或 Block。

当前 Schema 允许 allow 与 deny reason、critical finding 同时出现，也允许 deny 没有
reason。更严重的是该输出没有 `truncated`、coverage 或 detected language，因此调用方
无法判断 allow 是否来自完整扫描。建议先补齐 coverage 模型，再用条件 Schema 或 typed
validator 固定 verdict、reason、finding 的一致性。

## 6. Agent Sec native Schema

### 6.1 Agent Sec native request

![Agent Sec native request/v1](images/schemas/agent-sec-aw-provider-request-v1.svg)

一个请求入口通过 `operation` 承载 content、code、command 三种操作，便于复用现有
Python scanner。`source`、`include_low_confidence` 和 `language` 是不同操作的选项。

Schema 没有使用 `oneOf` 或 `if/then` 要求各 operation 必须携带对应字段。例如
`command_inspect` 缺少 language 仍能通过验证。建议按 operation 建立判别联合；若三种
操作将来生命周期不同，也可拆成三个 native endpoint，减少无效字段组合。

### 6.2 Agent Sec native response

![Agent Sec native response/v1](images/schemas/agent-sec-aw-provider-response-v1.svg)

`disposition` 表示进程已完成、跳过或出错，`findings_total` 和 `scanned_bytes` 供 Host
记录 meter。`verdict` 同时容纳 inspection verdict 与 command verdict。

统一 envelope 有利于复用，但联合枚举没有与 operation 绑定，content scan 可以返回
`allow`，`completed` 也可以不返回 verdict。`findings_total` 不必等于 findings 中 count
之和。建议响应回显 operation，按 operation 与 disposition 建条件分支，并以同一测试
向 JSON Schema validator 和 Python/Rust deserializer 验证。

## 7. Tokenless native Schema

### 7.1 Tokenless CompressionRequest

![Tokenless CompressionRequest/v1](images/schemas/tokenless-compression-request-v1.svg)

请求包含待压缩 `content`、调用 `seam`、关联标识以及三项 capabilities 开关。Host 根据
manifest 把 canonical artifact 映射为该结构，无需理解 Tokenless 算法。

当前 `additionalProperties=true`，多个字符串和嵌套字段没有大小上限。manifest 把 AW
的 environment instance id 映射到 Tokenless 的 `agent_id`，但后者原义是稳定 frontend
名称，例如 `claude-code`。应新增明确的 frontend kind/name，或在仅服务 COSH 时使用
稳定常量，不能复用不同语义的字段。

### 7.2 Tokenless CompressionResponse

![Tokenless CompressionResponse/v1](images/schemas/tokenless-compression-response-v1.svg)

`disposition` 区分 applied、passthrough、no_savings、timeout 等结果；`output` 是候选文本；
`before_tokens`、`after_tokens` 与 `tokenizer_id` 描述估算；`compressor_chain` 和
`stash_keys` 描述转换与恢复信息。

Schema 没有表达 `applied` 才能返回改变后的 output、`retrievable` 必须存在 stash_keys、
`error` 必须有 diagnostic 等状态不变量。数组与自由文本缺少界限，且 AW mapping 会丢弃
`content_type` 和 `stash_keys`。最优先修复仍是 `lossless` 语义转换，否则 COSH 会把不可
逆删字段的 candidate 当作满足强保证的结果采用。

## 8. 关联但不参与 Provider 的 Schema

### 8.1 Skill Ledger analyze result

![Skill Ledger analyze result/v1](images/schemas/skill-ledger-analyze-v1.svg)

该 Schema 描述 Agent Sec Skill Ledger 的离线分析结果。它包含总体 status、coverage、
scanner 结果、finding 与 error。本 PR 只修改 `$id`，不能把它当作 AW Ledger Schema。

该模型的 `coverage_complete` 值得 AW security output 借鉴。另一方面，finding 的
`metadata` 是无约束对象，不能用于要求 content-free 的 AW receipt 或 Ledger 路径。

### 8.2 COSH-NG E2E result

![COSH-NG E2E result](images/schemas/cosh-ng-e2e-result.svg)

该 Schema 记录一次分阶段 E2E 测试的运行信息、被测产物摘要、case 结果和 cleanup 状态。
本 PR 同样只修改 `$id`，它不参与 Provider 生效链。

结构分层和产物 SHA-256 是合理设计。`environment` 与 `metrics` 是无约束对象，`attempts`
允许 0，时间先后和 cleanup 状态也没有跨字段约束。若 `$id` 被外部工具当成缓存键或
Schema 身份，域名变更还需要迁移说明。

## 9. 跨 Schema 的矛盾清单

| 优先级 | 矛盾 | 影响 | 建议修复 |
| --- | --- | --- | --- |
| P1 | AW 与 Tokenless 的 `lossless` 定义不同 | 实际丢字段的结果可被 COSH 采用 | 保留 AW 强定义，在 Tokenless AW adapter 严格转换 |
| P1 | `language=auto` 实际固定按 Bash | Python 风险可漏检并返回 clean | 移除 auto 或实现检测、双扫描和 unknown 降级 |
| P1 | 扫描字节不是最终模型可见字节 | 结论、digest 与真实风险对象错位 | 在边界上固定单一 source bytes，并用 digest 贯穿 |
| P1 | Host 不做 payload Schema instance validation | native shape 漂移只能在映射或 typed decode 后发现 | admission 后缓存 validator，输入输出都验证 |
| P2 | `maxLength` 字符数与 Rust UTF-8 bytes 不同 | Schema-valid 数据可能无法反序列化 | 使用 ASCII pattern/专用 ID，或统一长度定义 |
| P2 | verdict 与 findings/coverage 无条件约束 | clean/allow 可与危险 finding 或截断并存 | 使用判别联合和 typed cross-field validation |
| P2 | metadata 中存在 Provider 自由文本 | receipt/Ledger 可能成为内容泄漏通道 | 使用注册表、专用标识类型及 manifest-derived metadata |
| P2 | canonical Schema 复制到 Provider 包 | 副本可能与权威文件漂移 | CI 重算逐字节一致性和 manifest digest |

## 10. Schema 驱动的修复顺序

### 阶段一：冻结公共语义

1. 为 artifact、digest、name、media type、transform id 定义公共词法与字节规则。
2. 冻结 `lossless`、`retrievable`、coverage、truncated 和 indeterminate 的准确含义。
3. 为每项 Capability 写出状态表，列明合法和非法字段组合。
4. 确认 Observe、Advise、Mediate 各自在什么时刻才算真正生效。

### 阶段二：让所有表示一致

1. 从同一 conformance vectors 验证 canonical Schema、Rust 类型和 Provider copy。
2. 为 native Schema、Python/Rust request model 和真实 CLI 添加同样的 vectors。
3. 由 CI 重算 Schema SHA-256，禁止人工复制摘要。
4. 给每个 breaking change 升 Capability 或 Schema version，不原地改变 v1 语义。

### 阶段三：落实运行时保证

1. Provider Host 对 canonical input、native request、native response、canonical output
   执行真实 instance validation。
2. Core 在采用前验证 source id、source digest、coverage 和 capability-specific invariant。
3. COSH 用同一 digest 绑定“被扫描”“被替换”“最终执行或入模”的字节。
4. Ledger 只保存受限、可验证的事实，不接收 Provider 控制的任意文本。

### 阶段四：建立最小真实链路

至少保留一条默认运行的测试：真实 COSH fixture → AW Hook → Core → Host → 真实 Tokenless
binary → canonical candidate → COSH replacement → content-free Ledger。测试应覆盖 applied、
passthrough、timeout、oversize、malformed output、不可逆删字段和 legacy Hook 冲突。

## 11. 图册生成与复核

14 张 SVG 由 `diagram-sources/schema-catalog.json` 和
`diagram-sources/render-schema-diagrams.mjs` 确定性生成。图中绿色区域表示当前可保留的
设计，橙色区域表示接口冻结前应讨论或修复的内容。生成脚本只使用内联 SVG、系统字体和
固定配色，不依赖网络资源。
