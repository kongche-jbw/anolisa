# PR 3 综合 Review 评论草稿

本页准备一份可直接提交到
[casparant/anolisa PR 3](https://github.com/casparant/anolisa/pull/3)
的 Review 评论。审查基线为
`8574ecb022ec9ffc68e1a71e30f2186b6ec81674`，头提交为
`42d07649409ecd5bb023056b28545efbd9325ef2`。

建议 Review 结论选择 `Request changes`。结论只针对合同语义和真实调用逻辑。构建、
打包、默认安装路径和 CI 触发范围可以另开后续问题，不作为本轮阻断依据。

## 可直接提交的评论

更正并更新我此前的 Review。撤回为 v1 新增 `unknown` 或 `indeterminate` verdict 的建议。
安全 verdict 应保持闭集；`clean` 需要完整覆盖，已验证前缀中发现的风险可以携带
`truncated=true`，没有可验证业务事实时才进入 Provider 终态、gap 或 degradation。

整体架构方向成立，建议继续保留。AW 维护稳定的 canonical Capability 合同，组件维护
自己的 native 协议，`provider.toml` 声明两者之间的映射，Core 负责计划、策略和结果
校验，Provider Host 负责发现、校验、执行与结果回传，COSH 负责候选选择与最终采用。这种职责
划分能够让 Tokenless、Agent Sec 和后续组件沿同一机制接入。

当前仍有几处合同声明与真实数据流不一致。它们会直接影响安全结论、候选采用和审计
可信度。建议本轮选择 `Request changes`，先固定下面的 v1 语义，再处理构建和产品化
工作。

### [P1] 安全检查结果没有绑定最终交付或执行的字节

COSH 在调用 Hook 前，会通过
[`to_redacted_json_with_schemas`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/cosh-ng/crates/cosh-core/src/hook.rs#L974-L988)
序列化并脱敏整个 Hook 输入。AW Hook 随后从这份脱敏数据中提取命令或工具响应，例如
[`adapt_post_tool_use`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw/crates/aw-cosh-hook/src/lib.rs#L165-L196)
处理的 `llmContent`。当 Hook 没有给出替换值时，COSH 仍会把原始
[`result.output`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/cosh-ng/crates/cosh-core/src/core.rs#L1540-L1562)
交给后续流程。

这样会出现两个内容版本。Agent Sec 检查的是脱敏副本，模型或工具收到的可能是原文。
安全结论、`source_digest` 和 Artifact 身份因此可能指向错误的字节。现有
[`ProviderReceipt`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw/crates/aw-contracts/src/provider.rs#L360-L395)
也没有保存被检查输入的 Schema 与摘要，事后无法证明检查对象与交付对象一致。

合并前需要建立并测试一条明确不变量。安全判断绑定的输入摘要必须与最终交给模型或
工具的字节摘要相同。若中间还有脱敏或其他 Hook 改写，需要明确在哪个阶段重新检查，
Receipt 如何记录最终输入身份，以及 COSH 如何记录最终采用或执行的摘要。

### [P1] `lossless` 在 AW 与 Tokenless 中代表不同保证

AW 将 `lossless` 定义为
[`保留全部源信息`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw/crates/aw-contracts/src/context.rs#L62-L71)。
Tokenless 的同名值表示
[`没有移除与任务有关的信息`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/tokenless/crates/tokenless-protocol/src/lib.rs#L365-L376)。
manifest 又把 native `reversibility` 直接映射到 canonical candidate，见
[`provider.toml`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/providers/tokenless/provider.toml#L138-L146)。

这个差异可以在实现中复现。Tokenless 会删除 `debug`、`trace`、空值等字段，只要没有
发生截断仍返回 `lossless`，见
[`json.rs`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/tokenless/crates/tokenless-compressors/src/json.rs#L284-L313)
和对应
[`测试`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/tokenless/crates/tokenless-compressors/src/tests/json_tests.rs#L19-L30)。
AW Hook 又把 `lossless` 作为采用候选内容的门槛，见
[`adopt_candidate`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw/crates/aw-cosh-hook/src/lib.rs#L566-L603)。

结果是已经丢失字段的输出仍能满足 AW 最强的可逆性保证，并成为 replacement。建议保留
AW 的强定义，在 Provider 适配层严格翻译 Tokenless 状态，并增加跨组件一致性测试。
任何标为 `lossless` 的 candidate 都必须能够恢复全部源信息。

### [P1] `allow_text_reencoding` 没有形成可验证的媒体类型闭环

canonical input 要求调用方声明
[`allow_text_reencoding`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw/crates/aw-contracts/schemas/context-projection-prepare-input-v1.schema.json#L49-L57)，
manifest 也把它映射到 Tokenless `replace_with_text`。Tokenless 会据此决定是否允许 Toon
文本表示，并在 false 时尽量保持 JSON 顶层形状。但是原 native
[`CompressionRequest`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/providers/tokenless/schemas/tokenless-compression-request-v1.schema.json#L5-L48)
没有 source media type，原 native
[`CompressionResponse`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/providers/tokenless/schemas/tokenless-compression-response-v1.schema.json#L5-L66)
也没有 output media type。manifest 最后无条件把 produced candidate 标成
[`text/plain`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/providers/tokenless/provider.toml#L128-L136)，
而 Core 的
[`validate_candidate`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw/crates/aw-core/src/lib.rs#L890-L901)
没有核对输入约束与 candidate media type。

因此，组件内部可能保持了 JSON 形状，AW 合同却不能证明输出仍是调用方允许的表示类型。
合并前应把 source media type 映射到 native request，让 Provider 显式返回 output media
type，再由 manifest 映射到 candidate。Core 必须拒绝
`allow_text_reencoding=false` 且 candidate media type 与 source 不同的结果。至少需要覆盖
`application/json -> text/plain` 允许路径和 `application/json -> application/json` 保持
路径的真实组件测试。

### [P1] `language=auto` 的 Schema 承诺与扫描实现不一致

canonical code input Schema 允许
[`auto`、`bash` 和 `python`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/providers/agent-sec-core/schemas/security-code-inspect-input-v1.schema.json#L46-L57)，
Core 在 PostToolUse 代码检查中固定发送
[`LanguageHint::Auto`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw/crates/aw-core/src/lib.rs#L828-L854)。
Agent Sec 只在值为 `python` 时使用 Python 扫描器，其他值一律进入 Bash 扫描器，见
[`handlers.py`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/agent-sec-core/agent-sec-cli/src/agent_sec_cli/aw_provider/handlers.py#L134-L144)。
Bash 路径仅提取部分内嵌 Python，见
[`scanner.py`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/agent-sec-core/agent-sec-cli/src/agent_sec_cli/code_scanner/scanner.py#L92-L106)。

普通 Python 代码可能只经过 Bash 规则并返回 `clean`。例如 Python 独有的反序列化规则
无法覆盖这种输入。合并前需要给 `auto` 一个可验证的含义。调用方可以提供确定语言，
也可以由 Provider 做检测或运行明确的多语言规则集合。任一必要扫描失败时不得返回
`clean`，而应把本次调用记录为 Provider failure、observation gap 或 gate degradation。
请补充纯 Python、纯 Bash、mixed 和扫描失败的合同测试。

### [P1] Schema 被声明为合同，但运行时没有校验四个数据阶段

Host admission 会检查 Schema 文件路径、大小、JSON 语法和摘要，见
[`manifest.rs`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw/crates/aw-provider-host/src/manifest.rs#L681-L790)。
调用时，Host 只对 canonical body 做摘要检查和字段映射，再把 native stdout 解析为普通
`serde_json::Value`，见
[`driver.rs`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw/crates/aw-provider-host/src/driver.rs#L30-L88)
和
[`native response 解析`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw/crates/aw-provider-host/src/driver.rs#L135-L160)。
canonical input、native request、native response 和 canonical output 都没有执行 JSON
Schema instance validation。

Schema 本身也允许互相矛盾的状态。content output 可以同时表示 `clean` 和 critical
finding，也可以表示 `truncated=true` 和 `clean`，见
[`security-content-inspect-output-v1`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/providers/agent-sec-core/schemas/security-content-inspect-output-v1.schema.json#L7-L65)。
command output 可以给出 `allow` 和 critical finding，或给出没有理由的 `deny`，见
[`security-command-inspect-output-v1`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/providers/agent-sec-core/schemas/security-command-inspect-output-v1.schema.json#L7-L70)。
native request 与 response 还把三个 operation 放进同一个字段袋，缺少能约束分支的
discriminator，见
[`request Schema`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/providers/agent-sec-core/schemas/agent-sec-aw-provider-request-v1.schema.json#L5-L30)
和
[`response Schema`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/providers/agent-sec-core/schemas/agent-sec-aw-provider-response-v1.schema.json#L5-L100)。

合并前需要确认 Schema 的权威性。如果 Schema 是运行时合同，应使用带 discriminator 的
联合类型或条件约束表达合法状态，并在四个阶段执行校验。跨字段求和等 Schema 不容易
表达的不变量，应由 Rust 或 Python typed validation 补足，并由同一组 conformance
vectors 保持一致。如果实际权威来自 typed model，文档也必须明确 Schema 的保证范围。

### [P1] `content-free` 目前只约束字段名，没有约束可持久化的值

Core 会把 Provider 返回的 `media_type`、`transform_chain` 和规则标识写入 Ledger，见
[`outcome.rs`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw/crates/aw-core/src/outcome.rs#L141-L194)。
Ledger admission 只递归禁止少量键名，见
[`admission.rs`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw/crates/aw-ledger/src/admission.rs#L173-L235)。
这些值所使用的 `BoundedName` 只限制非空、NUL 和 128 个 UTF-8 字节，见
[`common.rs`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw/crates/aw-contracts/src/common.rs#L32-L41)。
Provider 还可以通过 meter method 等自由文本把内容带进 Receipt，见
[`driver.rs`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw/crates/aw-provider-host/src/driver.rs#L827-L852)。

因此，输入片段或 secret 即使没有放在名为 `content` 的字段中，仍可能进入长期存储。
当前类型形状还不能证明 content freedom。建议把可持久化 metadata 收敛到 manifest 中
已接纳的闭集值、摘要或专用标识类型，避免从 native response 接收任意名称，并补充
secret 出现在 method、transform、media type 和 rule id 时的拒绝或投影测试。

Ledger 还需要证明记录的完整性与调用归属。一次 PostToolUse plan 中，每个计划的 Observe
step 都应由 observation 或明确 gap 覆盖，两者互斥；每个 invocation reference 必须与
Receipt 以及 Ledger header 的 attempt/tool scope 一致。Final adoption 应引用同一 plan、
同一组 invocation、source digest、candidate envelope digest 和最终 effective digest。
只过滤可疑字段名，或只记录一条孤立 Receipt，都不能证明完整调用链。

### v1 Schema 冻结前需要逐项确认的语义

1. 输入身份。被检查、被压缩、被执行和被交付的数据分别是哪一段字节，摘要在哪里
   计算，发生改写后由谁重新确认。
2. 可逆性。`lossless` 必须表示能够恢复精确源信息；`retrievable` 还需要 resolver、
   reference、权限和 TTL 合同。
3. 检查覆盖。`clean` 只表示一次完整成功扫描没有 finding。已验证前缀中发现的
   风险可以保留 `suspicious | sensitive + truncated=true`；失败、timeout、无实现或没有
   可验证 finding 的截断进入 Provider 终态、gap 或 degradation。
4. 状态组合。每个 operation 应有独立或可判别的 request、response 分支，禁止
   `allow + critical`、`clean + truncated` 等矛盾组合。
5. 表示类型。source media type、Provider output media type 与 candidate media type 必须
   连续可验证；`allow_text_reencoding=false` 必须由 Core 执行。
6. 字符与领域类型。名称需要统一字节上限和字符集合；`media_type`、rule ID、transform
   ID 等长期值需要专用类型或受控注册表。
7. 持久化边界。Receipt 和 Ledger 只保存闭集元数据、摘要和引用，candidate 正文只能
   沿即时调用路径返回。
8. 最终采用。Provider 产生 candidate 不等于 COSH 已采用。需要记录最终 Tool Result
   的 digest、采用决策及其关联的 Provider invocation。
9. Checkpoint。当前 one-shot Host 无法表达 generation、peer UID、socket instance 和
   effect reconciliation，不应在这些语义冻结前声明通用 AW Checkpoint Schema。

这些问题都可以在现有职责划分内修复，无需推翻 Provider 架构。详细字段图、真实
Tokenless 调用数据和架构分析可参考 fork 中的
[Schema 图册](https://github.com/kongche-jbw/anolisa/blob/chore/review/pr3-aw-reports/review-artifacts/pr-3-aw-provider/schema-reference_zh.md)、
[运行实例](https://github.com/kongche-jbw/anolisa/blob/chore/review/pr3-aw-reports/review-artifacts/pr-3-aw-provider/runtime-call-examples_zh.md)
和
[完整架构审查](https://github.com/kongche-jbw/anolisa/blob/chore/review/pr3-aw-reports/review-artifacts/pr-3-aw-provider/architecture-review_zh.md)。

本轮没有把构建、打包、默认安装路径、CI 变更检测和真实二进制 E2E 列为阻断项。这些
问题仍需跟踪，适合在合同语义确定后单独收口。

## Fork PoC 基线解决映射

fork 的
[`feat/aw/provider-e2e-poc`](https://github.com/kongche-jbw/anolisa/tree/feat/aw/provider-e2e-poc)
分支用于验证上述问题能否在现有 Provider 分层内解决。它不是对原 PR Review 要求的
替代，也不表示原 PR 头提交已经包含这些修复。

| Review 问题 | PoC 提交 | 当前结果 | 剩余边界 |
| --- | --- | --- | --- |
| 输入与最终字节未绑定 | [`1328cf30`](https://github.com/kongche-jbw/anolisa/commit/1328cf30)、[`601b5558`](https://github.com/kongche-jbw/anolisa/commit/601b5558) | Receipt 绑定 input/output；`context_adoption/v1` 再绑定 plan、candidate envelope 与 COSH 本地历史字节 | PreToolUse 仍需 exact executed-bytes digest；不声称远端模型已消费 |
| AW 与 Tokenless `lossless` 不同义 | [`414998b2`](https://github.com/kongche-jbw/anolisa/commit/414998b2) | 删除、清理或未追踪规范化都会降为 `unrecoverable` | `retrievable` resolver、reference 和 TTL 未冻结 |
| 表示类型约束无法验证 | [`8ecb1412`](https://github.com/kongche-jbw/anolisa/commit/8ecb1412) | native request/response 显式携带 input/output media type；Core 执行 `allow_text_reencoding` | `media_type` 注册表仍待冻结 |
| `auto` 实际只扫 Bash | [`d1d813d5`](https://github.com/kongche-jbw/anolisa/commit/d1d813d5) | `auto` 同时完成 Bash 与 Python 才返回 `mixed`；失败不返回 clean | 新语言的版本化覆盖规则未定义 |
| 四阶段没有实例校验 | [`6238842a`](https://github.com/kongche-jbw/anolisa/commit/6238842a)、[`4d47593b`](https://github.com/kongche-jbw/anolisa/commit/4d47593b) | Host 校验四阶段；canonical 状态表由 Schema 和 Rust 类型共同执行 | 需要长期维护跨语言 conformance vectors |
| Agent Sec native 字段袋 | [`d1d813d5`](https://github.com/kongche-jbw/anolisa/commit/d1d813d5) | request/response 已按 operation 和 disposition 判别；响应 operation 必须相关 | 新 operation 需要 native 协议版本演进 |
| Receipt/Ledger 可持久化任意文本 | [`1328cf30`](https://github.com/kongche-jbw/anolisa/commit/1328cf30)、[`5d7f6b62`](https://github.com/kongche-jbw/anolisa/commit/5d7f6b62)、[`8ecb1412`](https://github.com/kongche-jbw/anolisa/commit/8ecb1412) | meter method 来自已接纳 manifest；rule/reason 持久化为摘要；plan 完整覆盖并与 adoption 共用 invocation scope | `media_type` 和其他 taxonomy 仍需专用类型或注册表 |

PoC 的结果支持原 Review 判断。大方向无需推翻，主要问题来自合同语义没有贯穿到运行时
校验、最终字节和持久化边界。优先冻结这些语义后，再扩展 Provider 数量和 State
Provider 生命周期，风险最低。
