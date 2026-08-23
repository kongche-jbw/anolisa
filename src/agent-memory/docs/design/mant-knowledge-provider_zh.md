# ManT Knowledge Provider 设计

[English](mant-knowledge-provider.md)

Knowledge Provider 边界让 Agent Memory 可以按需、聚焦地访问其他系统拥有的
文档。ManT 只是这个边界的一种适配器。它不属于存储依赖，Agent Memory
不会安装或下载它，本地 TaskState 与证据召回也不依赖它。

## Provider 无关契约

`KnowledgeProvider` 是同步的 `Send + Sync` trait，包含三个操作：

| 操作 | 契约 |
|---|---|
| `descriptor` | 执行在线能力协商，返回类型化的身份、版本、协议和聚焦能力 |
| `health` | descriptor 成功时返回 `healthy`，所有类型化协商失败都映射为 `degraded` |
| `query` | 把一个有界 `KnowledgeQuery` 解析为有界的 `KnowledgeItem` |

`KnowledgeQuery` 必须指定一个文档和唯一一种聚焦 selector，可选 literal
search、单条目 explain 或分节 excerpt。接口没有整篇文档 selector。调用
provider 前，query validation 会限制单项及组合输入大小、selector 数量、
结果数量和 excerpt 字节数。

每个 item 只包含 `KnowledgeRef`、可选的有界标题、有界 excerpt、响应
fingerprint 和可选相关度。ref 保留 provider、document、聚焦 selector、
检索时间以及用于过期检查的 fingerprint。整篇 manual 和 provider 数据库
始终由 provider 持有。

Provider 解析只建立来源，不证明内容为真。返回 item 进入 Memory admission
边界时仍是 Candidate 和 untrusted data。Runtime 必须保留固定的不可信数据
wrapper，不能因为内容经过 ManT 解析就把它提升为 Verified 或 Normative。

## ManT v0.9 适配器

`MantCliProvider` 只会直接执行显式配置的 executable path，不查找、安装或
更新 ManT。它不会调用 shell，所有用户控制的 document 和 selector 值都以
JSON 写入 stdin，不进入进程参数。

每次聚焦查询前，适配器执行：

```text
mant --protocol-version --compact
```

返回的 JSON descriptor 必须精确声明 `mant.cli/v0.9`、
`mant.request/v0.9`、`mant.excerpt/v0.9` 和 `mant.search/v0.9`。
未知的新增 descriptor 字段会被忽略。这样可以容纳兼容的 metadata 扩展，
同时拒绝不同的请求或响应 schema。

聚焦查询执行：

```text
mant --request-json --format json --compact
```

适配器向 stdin 写入唯一一个 compact JSON 请求，并显式检查 ManT native
request 的 65,536-byte 上限。Search 始终使用 literal 模式并限制在可见
document scope 内。Explain 和 excerpt 响应必须声明
`mant.excerpt/v0.9`，search 响应必须声明 `mant.search/v0.9`。
适配器只从 selections 或 matches 等聚焦响应集合提取内容，并再次限制
最终允许进入 Memory 的 excerpt。

## 进程与失败边界

每次 probe 和 query 都有硬性 wall-clock deadline。适配器创建独立的
process group，并发排空 stdout 和 stderr 到有界 buffer，超时后杀死整个
group。这样 CLI 拉起的 helper process 无法在主进程退出后继续占用输出管道
或延长请求生命期。

只有不超过配置上限、并符合协商 schema 的有效 JSON stdout 才会被接受。
stderr 会被有界排空以保证进程安全，随后直接丢弃，绝不进入 error message、
memory item、日志或模型上下文。Safe error 也不包含 executable path、query
和 document 内容。

Health 采用明确的安全降级：

| 条件 | 类型化状态 |
|---|---|
| executable 缺失或不可执行 | `degraded / unavailable` |
| protocol 或必要 schema 不同 | `degraded / incompatible` |
| 超过 deadline | `degraded / timeout` |
| stdout 或 stderr 超过上限 | `degraded / resource_exhausted` |
| JSON 或 response schema 无效 | `degraded / malformed_response` |

Knowledge provider 降级不会阻塞本地 TaskState 或证据召回，也不能被呈现成
完整的 knowledge hit。Runtime 在没有 provider context 的情况下继续，并通过
health 或可观测性暴露类型化降级原因。

## Local broker 与 Cosh binding

`LocalMemoryBackend::open_with_knowledge` 接受 provider 无关的 binding；默认
`open` 路径不依赖 knowledge。普通 turn 中，broker 从 prompt 选择一个聚焦的
literal，依次 admission 经审阅的 TaskState、Candidate knowledge 和现场 tool
evidence。三条 lane 共享同一个 item、byte、token budget，并写入同一条
RecallTrace。Provider 失败时，view 使用 `local_only_knowledge_degraded`，记录
类型化原因，同时继续返回符合条件的本地 state 与 evidence。

Cosh one-shot Hook 会从可信 PATH 发现已经安装的 `mant`。
`ANOLISA_MANT_PATH` 可指定显式 executable，
`ANOLISA_MEMORY_MANT_DOCUMENT` 选择 logical document，默认是 `bash`，
`ANOLISA_MEMORY_MANT=off` 可关闭 binding。这些值属于可信 host 配置，不是
模型参数。ManT 不存在表示 provider 未加载，不表示本地 memory 故障；Agent
Memory 绝不会运行安装或更新命令。

## Fingerprint 与生命周期

v0.9 adapter 对已经有界的聚焦 JSON response 计算 `fnv1a64` change
fingerprint。它是确定性的过期探测器，不是密码学完整性证明。Memory 可以
持久化 ref、selector、fingerprint 和自身真正 admission 的有界 excerpt。
刷新内容时必须重新查询 provider，不能把 excerpt 逐步变成脱离 provider 的
manual 副本。

适配器不缓存 descriptor 或查询结果。因此 executable 被替换或协议升级后，
下一次操作即可发现；代价是每次 query 多执行一次 probe。未来可以在同一
trait 后增加 supervised provider 或 cache，但必须保留明确的过期策略与类型化
降级。

## 验证

`knowledge_provider_test.rs` 覆盖 provider-neutral fake、使用协商后 one-shot
JSON 形状的 fake executable、排除无关整篇 manual 字段的聚焦提取、缺失及
不兼容 executable、process group timeout、输出上限和聚合 query 上限。支持
新的 ManT protocol family 时必须显式修改适配器和 fixture；当前实现不会猜测
面向人的 CLI 文本输出。
`local_backend_test.rs` 还会验证合并选择和 local-only degradation，
`cosh_hook_wire_test.rs` 则让真实 one-shot Hook 进程连接 fake ManT v0.9
executable，验证端到端接入。
