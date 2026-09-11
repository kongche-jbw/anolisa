# SecCore 原生协议适配

[English](sec-core-adapter.md)

`aw-sec-core` 将 post-tool `security.content.inspect/v2` 请求映射到现有 SecCore PII CLI 协议，并校验原生结果。它不启动进程、不包含扫描器，也不依赖 Core。Host 使用该库，无须依赖 SecCore 内部使用哪种语言。

## 请求与结果边界

`PiiRequest::new` 验证 AW 输入并保留精确原文。`args()` 与 `stdin()` 描述以下原生调用：

```text
agent-sec-cli scan-pii --stdin --format json --source tool_output
```

请求要求包含低置信度结果时，追加 `--include-low-confidence`。输入不放进 argv。映射不覆盖用户规则配置、不指定固定 detector、不关闭 middleware，也不请求原始或脱敏证据片段。原生命令继续负责正常配置及安全事件生命周期。

`project(exit_code, stdout)` 校验原生响应后才生成 AW inspection。当前核对的 profile 是版本 0.12.0 的 `agent-sec-cli scan-pii`，黄金向量记录准确原生源码 revision。版本字符串本身不能证明兼容；Host 必须准入受支持的制品和配置，将响应绑定到对应调用，并拒绝协议漂移。

| 原生结果 | AW 结果 |
| --- | --- |
| 完整 `pass` 且没有 findings | `clean` |
| 完整 `warn` | `suspicious`；warning findings 映射 medium severity |
| 完整 `deny` | `sensitive`；denying findings 映射 high severity |
| 非零退出、扫描失败、输出格式或字段关系错误 | 映射错误，无 inspection |
| 部分扫描或自定义规则处理降级 | 映射错误，不声明完整检查 |

Findings 保留合法规则 ID，按相同 type/category/severity/confidence 分组聚合。原生 `custom` 映射为 AW `other`，不透传证据片段、span 或任意 metadata。置信度低于 0.5 为 low、低于 0.8 为 medium，其余 high。限制输出大小和 findings 数量，不静默裁剪来满足 schema。

Coverage 的 ruleset IDs 标识当前经评审的协议 profile，以及加载时原生自定义规则文件的摘要。Profile ID 不是内置规则的计算摘要，制品准入仍由 Host 负责。

原生输出提供扫描字节数，但没有输入摘要。Mapper 对照保留的 UTF-8 原文检查 coverage，使用已验证请求的 digest；它不能认证同长度响应替换，Host 必须建立请求与响应的绑定。纯映射校验也无法证明可执行程序实际运行了宣称的 detector。

自定义规则无效、运行错误、预算耗尽或自定义 findings 截断都形成可见映射错误，即使原生 CLI 继续执行内置规则。这样保留原生 CLI 行为，同时避免 AW 声称完整执行了配置中的检查。自定义规则文件不存在属于受支持配置，不是错误。

## Host 与安全职责

Host 负责进程或 daemon transport、制品/配置准入、超时、输出收集上限、子进程回收和 AW receipt 构造。本库收到输出后的大小检查不能限制调用方此前的内存分配。不提供自动 fallback、重试、hook 安装或最终 dispatch。正常映射/Provider 失败应形成 failed receipt；无法形成可信终态回执时才返回 Host transport error。

内容 findings 是观测事实。检查完成不授予执行权限、不安装 OS 防护，也不证明审计已持久落盘。SecCore 保留原生 audit/trace 所有权，AW Journal 记录编排事实。安全引擎迁移由 SecCore 负责；保留原生行为应能通过相同适配测试而无需修改 Core，接口版本变更则必须明确评审适配兼容性。

## 验证与原生 oracle

AW 常规检查执行非空 `pii` integration target，其中包含真实原生 CLI 的冻结输出。运行库和日常 Rust 测试不需要 Python scanner。合成反例覆盖非法记录、coverage、detector 降级及信息披露边界。

AW 门禁还运行两个仅依赖标准库的 oracle 自测，确保转义文本和禁止字段的审计检查有效。

`crates/aw-sec-core/tests/regenerate_pii.py` 使用原生项目 Python 3.11.6 和锁定依赖重跑真实 CLI。在仓库根使用该环境的 Python：

```bash
python src/aw/crates/aw-sec-core/tests/regenerate_pii.py
```

脚本比较稳定结果，覆盖内置/用户规则及置信度选择，并核验每次调用恰好生成一条脱敏原生安全事件。仅注入自定义规则文件位置，不替换 HOME、middleware 或 scanner。临时数据放在 AW `target`，退出时删除，`elapsed_ms` 归零。只有有意重生成经评审的向量时使用 `--write`；不能不检查兼容影响就刷新期望值以接受漂移。该 oracle 验证 Python 原生 CLI 入口，不是已安装二进制或完整 Agent/Host 会话。
