# PR 3 综合 Review 评论草稿

本页准备一份可直接提交到
[casparant/anolisa PR 3](https://github.com/casparant/anolisa/pull/3)
的 Review 评论。审查基线为
`8574ecb022ec9ffc68e1a71e30f2186b6ec81674`，头提交为
`42d07649409ecd5bb023056b28545efbd9325ef2`。

建议 Review 结论选为 `Request changes`。这项结论只针对合同语义和实际调用逻辑。
构建、打包、安装路径和 CI 触发范围可以另开后续问题，不作为本轮阻断依据。

## 可直接提交的评论

整体架构方向成立，建议继续保留。AW 维护稳定的 canonical Capability
合同，组件维护自己的 native 协议，`provider.toml` 负责声明映射，Core 负责计划、
策略和候选结果选择，Provider Host 负责发现、校验、执行与结果回传，COSH 负责最终
采用。这种职责划分能够让 Tokenless、Agent Sec 和后续组件沿同一套机制接入。

当前仍有几处合同声明与真实数据流不一致。它们会直接影响安全结论、候选结果采用和
审计可信度。我建议本轮选择 `Request changes`，先固定下面的 v1 语义，再继续处理
构建和产品化工作。

### [P1] 安全检查结果没有绑定最终交付或执行的字节

COSH 在调用 Hook 前，会通过
[`to_redacted_json_with_schemas`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/cosh-ng/crates/cosh-core/src/hook.rs#L974-L988)
序列化并脱敏整个 Hook 输入。AW Hook 随后从这份脱敏数据中提取命令或工具响应，
例如
[`adapt_post_tool_use`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw/crates/aw-cosh-hook/src/lib.rs#L165-L196)
处理的 `llmContent`。当 Hook 没有给出替换值时，COSH 仍会把原始
[`result.output`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/cosh-ng/crates/cosh-core/src/core.rs#L1540-L1562)
交给后续流程。

这样会出现两个内容版本。Agent Sec 检查的是脱敏副本，模型或工具收到的可能是原文。
安全结论、`source_digest` 和 Artifact 身份因此可能指向错误的字节。现有
[`ProviderReceipt`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw/crates/aw-contracts/src/provider.rs#L360-L395)
也没有保存被检查输入的 Schema 与摘要，事后无法证明检查对象与交付对象一致。

合并前请建立并测试一条明确不变量。安全判断所绑定的输入摘要，应与最终交给模型或
工具的字节摘要相同。若中间还有脱敏或其他 Hook 改写，也需要明确哪个阶段重新检查，
以及 Receipt 如何记录最终输入身份。

### [P1] `lossless` 在 AW 与 Tokenless 中代表不同保证

AW 将 `lossless` 定义为
[`保留全部源信息`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw/crates/aw-contracts/src/context.rs#L62-L71)。
Tokenless 的同名值表示
[`没有移除与任务有关的信息`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/tokenless/crates/tokenless-protocol/src/lib.rs#L365-L376)。
manifest 又把 native `reversibility` 直接映射到 canonical candidate，见
[`provider.toml`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/providers/tokenless/provider.toml#L138-L146)。

这个差异已经能在实现中复现。Tokenless 会删除 `debug`、`trace`、空值等字段，
只要没有发生截断仍返回 `lossless`，见
[`json.rs`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/tokenless/crates/tokenless-compressors/src/json.rs#L284-L313)
和对应
[`测试`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/tokenless/crates/tokenless-compressors/src/tests/json_tests.rs#L19-L30)。
AW Hook 又把 `lossless` 作为采用候选内容的门槛，见
[`adopt_candidate`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw/crates/aw-cosh-hook/src/lib.rs#L566-L603)。

结果是已经丢失字段的输出仍能满足 AW 最强的可逆性保证，并成为最终 replacement。
建议保留 AW 当前的强定义，在 Provider 适配层严格翻译 Tokenless 状态，并增加跨组件
一致性测试。凡是标为 `lossless` 的 candidate，都应能恢复全部源信息。

### [P1] `language=auto` 的 Schema 承诺与扫描实现不一致

canonical code input Schema 允许
[`auto`、`bash` 和 `python`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/providers/agent-sec-core/schemas/security-code-inspect-input-v1.schema.json#L46-L57)，
Core 在 PostToolUse 代码检查中固定发送
[`LanguageHint::Auto`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw/crates/aw-core/src/lib.rs#L828-L854)。
Agent Sec 只在值为 `python` 时使用 Python 扫描器，其他值一律进入 Bash 扫描器，见
[`handlers.py`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/agent-sec-core/agent-sec-cli/src/agent_sec_cli/aw_provider/handlers.py#L134-L144)。
Bash 路径仅提取部分内嵌 Python，见
[`scanner.py`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/agent-sec-core/agent-sec-cli/src/agent_sec_cli/code_scanner/scanner.py#L92-L106)。

普通 Python 代码可能因此只经过 Bash 规则并返回 `clean`。例如 Python 独有的反序列化
规则无法覆盖这种输入。合并前需要给 `auto` 一个可验证的含义。调用方可以负责给出
确定语言，也可以由 Provider 做检测或多语言扫描。无法证明覆盖范围时，结果应表达
`unknown` 或 `indeterminate`，不应返回确定的 `clean`。请补充纯 Python 与纯 Bash
的合同测试。

### [P1] Schema 被声明为合同，但运行时没有校验四个数据阶段

Host admission 会检查 Schema 文件路径、大小、JSON 语法和摘要，见
[`manifest.rs`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw/crates/aw-provider-host/src/manifest.rs#L681-L790)。
调用时，Host 只对 canonical body 做摘要检查和字段映射，再把 native stdout 解析为
普通 `serde_json::Value`，见
[`driver.rs`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw/crates/aw-provider-host/src/driver.rs#L30-L88)
和
[`native response 解析`](https://github.com/casparant/anolisa/blob/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw/crates/aw-provider-host/src/driver.rs#L135-L160)。
canonical input、native request、native response 和 canonical output 都没有进行 JSON
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

合并前请确认 Schema 的权威性。如果 Schema 是运行时合同，建议用带 discriminator 的
联合类型或条件约束表达合法状态，并在四个阶段执行校验。如果实际权威来自 Rust 或
Python 类型，则文档需要降低 Schema 的保证范围，并说明跨语言一致性由什么测试维护。
我更倾向于前一种选择，因为当前架构已经把 Schema 摘要写入 manifest 和 Capability
Graph，调用方会自然地把它理解为可执行合同。

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
当前类型形状还不能证明 content freedom。建议把可持久化 metadata 收敛到 manifest
中已接纳的枚举或专用标识类型，避免从 native response 接收任意名称，并补充 secret
出现在 method、transform、media type 和 rule id 时的拒绝测试。

### v1 Schema 冻结前需要逐项确认的语义

1. 输入身份。被检查、被压缩、被执行和被交付的数据分别是哪一段字节，摘要在哪里
   计算，发生改写后由谁重新确认。
2. 可逆性。`lossless` 是否要求逐字节恢复，还是只保留任务相关信息。相同枚举值不能
   在 Provider 和 AW 中表达不同保证。
3. 检查覆盖范围。`auto`、`clean`、`truncated`、`unknown` 和 `indeterminate` 的关系
   需要写成可测试的不变量。
4. 状态组合。每个 operation 应有独立或可判别的 request、response 分支，禁止
   `allow + critical`、`clean + truncated` 等矛盾组合。
5. 字符与长度。Rust `BoundedName` 使用 128 个 UTF-8 字节，部分 Schema 的
   `maxLength` 使用 256 个 Unicode 字符。两边需要统一计数单位和字符集合。
6. 持久化边界。哪些值能够进入 Receipt 和 Ledger，哪些值只能存在于瞬时 outcome，
   需要由专用类型和校验规则证明。
7. Schema 权威性。需要明确实例校验发生在哪一层、失败如何 settle、Schema 升级如何
   保持兼容，以及 typed model 与 Schema 如何共享一致性测试。

以上问题都可以在现有职责划分内修复，无需推翻 Provider 架构。详细字段图、真实
Tokenless 调用数据和 POC 对照可参考 fork 中的
[Schema 图册](https://github.com/kongche-jbw/anolisa/blob/chore/review/pr3-aw-reports/review-artifacts/pr-3-aw-provider/schema-reference_zh.md)、
[运行实例](https://github.com/kongche-jbw/anolisa/blob/chore/review/pr3-aw-reports/review-artifacts/pr-3-aw-provider/runtime-call-examples_zh.md)
和
[完整架构审查](https://github.com/kongche-jbw/anolisa/blob/chore/review/pr3-aw-reports/review-artifacts/pr-3-aw-provider/architecture-review_zh.md)。

本轮没有把构建、打包、默认安装路径、CI 变更检测和真实二进制 E2E 列为阻断项。
这些问题仍需跟踪，适合在合同语义确定后单独收口。

