# Tokenless 投影 Host

[English](tokenless-host.md)

`aw-tokenless-host` 为 `context.projection.prepare/v2` 提供可嵌入的 Linux
Provider。它调用现有 Tokenless CLI，返回经校验的候选与回执。原生 hooks、结果替换、
采用核验及计费度量属于后续接线工作。

## 职责划分

执行路径为 Core → Tokenless Host → 原生 `compress` → 候选/回执 → Core Journal。
Contracts 和 Core 不依赖 Tokenless。Host 仅引入现有纯协议 crate
`tokenless-protocol`，复用原生类型与估算一致性规则；压缩引擎仍在原生可执行文件中。

第二个原生 Host 与 SecCore 共用 `aw-host-process`，由其承接已有启动 `Config`、
pin、流量限额、取消与进程组清理。版本准入、协议映射、descriptor 和 receipt
继续归具体 Host。SecCore 重导出相同配置类型，保持序列化配置与 manifest digest
不变；没有新增泛型 Provider 框架或另一套 supervisor。

AW 门禁约束八个 workspace 成员、原生协议的精确路径及纯依赖边界。
协议 crate 或 Tokenless workspace manifest 变化会触发 AW CI。
原生引擎变化需要执行下方的显式验收，冻结向量回放无法证明新引擎兼容。

## 准入与原生 profile

`TokenlessHost::new` 接收与 SecCore 同形的操作者配置，详见
[启动来源与进程生命周期](sec-core-host_zh.md)。`new_cancellable` 使版本探测与
每次调用共用调用方持有的取消状态。构造器要求独立提供的程序 SHA-256，以及精确版本响应
`tokenless 0.8.1\n`。manifest 绑定完整配置、原生版本、协议 profile 与能力。
Provider 声明 `authority: advise`、`guarantee: declared`，仅支持 `post_tool`。

完整子进程环境必须显式包含：

```text
TOKENLESS_STATS_ENABLED=0
TOKENLESS_SLS_ENABLED=0
TOKENLESS_COMPRESSION_ENABLED=1
```

缺少或冲突值会使准入失败。三个原生开关共同避免加载用户默认配置，并关闭统计和
SLS 写入。Host 通过 stdin 向 `tokenless compress` 发送 protocol 2 `post_tool`：
`result_kind: tool`、`status: success`、`output_optimization: none`、可替换输出、
`recovery.kind: none`。该 profile 不需要 stash；请求不会执行工具命令。程序与所选文件的完整 SHA 校验消耗
同一个调用预算，部署时应按实际二进制大小配置。

接入方须先确认工具成功完成：AW artifact 带有来源，但没有退出状态。准入要求
`origin: command_output`、`media_type: text/plain`、精确原文及摘要、工具名、
session/tool-use 身份，并显式接受 `unrecoverable`。不支持的来源和恢复要求明确失败，
不会把未知来源猜作 API response。

## 候选语义与失败处理

原生 `lossless` 表示保留任务相关信息，不证明能逐字节恢复 AW 原始 artifact。
因此本 profile 将符合要求的候选标为 AW `unrecoverable`，恢复模式为 `none`。
后续要声明 lossless/retrievable，必须先有真实 decoder 或授权 resolver 及恢复测试。

映射核对版本、operation、归属、disposition、恢复声明、转换操作、token 估算一致性及
实际 UTF-8 缩减，拒绝畸形或矛盾响应。候选绑定原始 artifact ID 与 digest，保留文本
媒体语义，并记录原生转换名称。最终由 `Registry::validate_result` 联合验证回执和输出。
只计量实际原文/候选字节数；原生估算不是模型计费或已核验收益。

固定 command-output/no-recovery profile 接受 `terminal_cleanup` 和 `json_cleanup`；
`toon` 与 `tabular_compaction` 还要求显式允许文本重编码。其他生命周期/来源路由的操作
及有损删减路径会被拒绝。原生 `Grep` 的 command output 没有可用压缩路由，其 applied
声明同样无效。

| 原生结果 | AW 结果 |
|---|---|
| 满足声明约束的有效 applied 候选 | `produced`，绑定候选与回执 |
| passthrough/no savings/recovery unavailable 且逐字保留原文 | `bypassed`，无替换输出 |
| 原生错误、超时、矛盾响应、pin 漂移或预算耗尽 | `failed`，无替换输出 |
| invocation 或 Provider 绑定无效 | 压缩调用前返回 Host error |

可选投影步骤可设置 `on_failure: preserve`。保留原文不意味着通过安全检查。
若应用要求安全策略，应在投影之前配置 required 检查步骤及 `reject_plan` 失败处理；
该步骤失败时 Core 在压缩前停止。Host 不自行插入策略或切换备用 Provider。

进程限额和清理沿用 SecCore Host 的已说明边界，包括所选文件 TOCTOU、逃离进程组的
后代与不可捕获信号。本 profile 不增加沙箱或最终 dispatch 权限。Journal 绑定执行元数据；
原文和候选的保留策略、候选是否交付仍由接入方决定。

## 验证与原生验收

正常 `scripts/check.py` 执行共享进程/SecCore 回归、真实本地协议测试进程、冻结原生
向量、Core 失败/顺序/Journal 测试及 oracle 自测，不构建另一份 Tokenless 引擎或运行真实 Agent。

经审阅的[原生向量](../../tests/fixtures/tokenless-native.json) 覆盖 JSON cleanup、
短文本、Unicode/换行和无收益。在仓库根目录显式验收原生构建：

```bash
cargo build --locked --manifest-path src/tokenless/Cargo.toml -p tokenless-cli
python3 -B src/aw/tests/tokenless_oracle.py --binary "$PWD/src/tokenless/target/debug/tokenless"
AW_TOKENLESS_NATIVE_BINARY="$PWD/src/tokenless/target/debug/tokenless" \
  cargo test --locked --manifest-path src/aw/Cargo.toml -p aw-tokenless-host \
  --test projection frozen_real_tokenless_responses_map_without_a_runtime_dependency -- --exact
```

oracle 将实际退出码和完整 JSON 与全部冻结向量对照，并检查无原生持久数据产生。
显式配置后的 Rust 测试进一步经过真实 Host，包括准入、pin 和 receipt；未配置环境变量时
使用本地协议测试进程。两条路径均在 AW 构建目录内创建临时数据并回收自有资源，
不修改 HOME 或 Agent 配置。

升级原生组件时共同审阅行为差异与映射，明确更新固定版本/profile 和冻结向量，重跑两项
验收。版本文本本身不是兼容证据；生成候选仍不证明宿主采用或实际模型收益。
