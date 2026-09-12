# Qoder 投影与已记录采用

[English](qoder-adoption.md)

显式 Qoder 路径在必需 SecCore 检查后返回 Tokenless 候选，并记录独立 local_history 观察。
复用已有 Adapter、Core、两个原生 Host 和 FileJournal。Contracts 保持纯校验；
Core 不依赖 Qoder、SecCore 或 Tokenless 实现。

## 责任与执行

`aw-hook-cli` 内按职责组织 `projection` 组合、`records` 私有 context、`history` 固定
历史读取和 `adoption` 观察/查询；薄 `aw-adoption-cli` 调用同一库。不新增第九个 crate、
第二套 Journal、插件安装器或后台 watcher。

1. 原生版本探测前，验证存活 owner/scope、成功结构化 Bash 结果、显式 unrecoverable/保留策略
   与声明历史文件，捕获 device/inode、前缀字节长度/摘要和输入。
2. 用不同 ID 注册已有 SecCore 和 Tokenless Host。一个计划串起必需检查与可选投影，
   两步均用 `reject_plan`；gap 保留原文并停止后续调用。
3. 串行执行、共享取消。每次调用预算独立；投影绝对 deadline 包含两步预算。
   SecCore 发现仍是观察，不批准或阻断最终工具派发。
4. 暴露替换文本前，私有 context 保留完整调用、输出、plan、boundary 和原始 Journal tip，
   对照完整计划及 Journal 校验；包括持久 invocation start 后、dispatch 前取消的合法情况。
5. 仅 produced 候选和 proceed 计划返回 Qoder `updatedToolOutput`；stdout 成功传输后
   单独写 marker。交付失败不追加第二个响应，也不声称采用。stdout 使用无缓冲非阻塞
   写入、五秒期限及取消检查，退出时恢复继承的描述符标志。

[Qoder 官方 hooks](https://docs.qoder.com/cli/hooks) 说明替换位置及原生组合规则。
固定历史 profile 为 `qoder-cli-1.1.47/jsonl-v1`，内部 JSONL 行结构不是官方稳定 API。
launcher 须验证安装版本并注册同步单轮 hook；本代码不安装或托管 Agent。

## 独立观察与持久证据

`observe` 仅读声明的当前用户所有、非末端符号链接普通文件，总量上限 16 MiB，
原生 JSON 每行上限 1 MiB。inode 和原前缀须不变。完整扫描快照，按准确 session/tool ID/cwd/input
及 `isSidechain: false` 匹配唯一有序 assistant `tool_use`、user `tool_result`。
成功结果可省略 `is_error` 或为 false，其他值拒绝。结果文本必须位于捕获前缀之外。
tool_input 可为对象或无重复键的 JSON 编码对象；调用行可已在前缀中，也可在结果前追加。

观察进程 wall clock 记录观察时间并检查 receipt 至观察的配置窗口，不用原生行 timestamp
证明 hooks 完成时间。缺失/迟到无采用结论；损坏、歧义、错绑定明确失败，不自动重试。

保留 fact 含 binding 摘要、快照摘要/大小、匹配原始行及行号/偏移/摘要、观察时间。
同一 Core FileJournal 使用由 context 摘要和 observation kind 派生的域隔离 key，追加 fact 摘要；
持久 ACK 后才将 fact 和外部 Journal tip 写入不可变私有同级文件。Core 只存元数据，不存行正文。
重复或中断 claim 继续占用，不通过盲重试恢复；外部 tip 未存成功时，query 无 committed 观察。

`query` 只读打开已有私有存储，验证原始执行和独立 observation 链，重放匹配行，再调用
`Registry::validate_plan_adoption`。不创建目录、同步文件、启动 Provider 或重新打开 live history。
重放只验证过去扫描中保留的匹配行；唯一性由 observer 当时完整扫描确认，不独立重扫或认证当前历史。
摘要不能对抗同时改写全部可信副本的主体。

## 结果含义

`prepared`、`returned`、`observation_status` 分别描述事实。缺 stdout marker 不必否定独立历史匹配。
adopted 要求准确候选与完整 proceed 计划；preserved 表示准确原文；overridden 表示其他文本，
归属收益为零。无候选且无历史仍为 unverified。
`ledger_status: committed` 必须指 observation 自身持久 ACK，不能借用原执行 ACK。
`candidate_digest` 按已有合同绑定完整输出 envelope。

查询证明固定为 `local_history`、`recorded_snapshot`。字节差只对应当时表示，不证明 AW 因果效果、
model request 交付、分词或计费；之后仍可能变换。本阶段无 model_request 证明、恢复、最终安全门禁
或默认产品启用。

## 验证与限制

AW 门禁覆盖真实本地协议 peer、固定 Tokenless 原生输出、合成历史、保留/覆盖/缺失观察、
损坏、重复事件、取消及交付失败；读取器成功/error 形状还与保留的 Qoder 1.1.47 合成工作负载
真实历史对照。这些检查不替代准确版本和注册配置下的真实 Qoder 验收；单独原生路径确认前
profile 保持实验性。明文保留及父路径隔离由接入 owner 负责，不新增自动保留策略或 GC。

配置、命令与禁用方式见[用户指南](../../../../docs/user-guide/zh/user-entrypoint/aw.md)。
