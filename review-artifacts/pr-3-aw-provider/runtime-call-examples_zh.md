# AW Provider 真实调用实例

本文通过 Tokenless、Agent Sec 和 Checkpoint 三条链路解释运行时字段。阅读者只需要了解
JSON、进程、文件和 SHA-256 摘要。示例区分原 PR、fork PoC 已实现基线和仍待完成的接线。

参考分支为
[`kongche-jbw/anolisa:feat/aw/provider-e2e-poc`](https://github.com/kongche-jbw/anolisa/tree/feat/aw/provider-e2e-poc)。
原 PR 固定头提交为 `42d07649409ecd5bb023056b28545efbd9325ef2`。fork PoC 与最终
VM 证据固定为 `5ebfc0b3905fa2f5f74aff2da4aec2b3be639647`。

配套交互图如下。

- [Tokenless 生效时序](provider-effect-sequence.html)
- [Agent Sec 命令检查时序](security-command-call.html)
- [Checkpoint State Provider 时序](checkpoint-create-call.html)
- [Provider 总体架构](provider-effect-architecture.html)

## 先分清五种数据

| 数据 | 是否含正文 | 谁产生 | 谁使用 |
| --- | --- | --- | --- |
| canonical input | 是 | AW Core | Provider Host |
| native request | 是 | Provider Host | 组件 Provider |
| native response | 可以 | 组件 Provider | Provider Host |
| transient candidate | 可以 | Provider Host | AW Core 与 COSH |
| receipt 与 Ledger record | 否 | Host、Core、COSH | 审计与运维 |

无状态 Provider 的完整往返方向如下。

```text
请求
COSH -> AW Core -> Provider Host -> Provider

返回
Provider -> Provider Host -> AW Core -> AW Ledger post_tool_use_plan
                                      -> COSH adopted or preserved
                                      -> AW Ledger context_adoption referencing plan
```

Provider Host 必须把实际处理结果返回 Core。Receipt 只保存调用身份，不能代替 candidate。
Candidate 只是一份建议，也不能代替 COSH 的最终采用事实。

## Tokenless 实例

### Source Tool Result

PoC fixture 模拟 `list_recent_builds` 返回六条构建记录。`llmContent` 是 693 个 UTF-8 字节，
SHA-256 为
`01202f4b809e4ffee777b3b7a62ca0d5007b99738c0abbc537fd1cc29d7422e1`。

下面固定一组 artifact 与 scope ID，便于读者沿整条 trace 对照字段。实际运行会生成新 ID。
因此 source 和 candidate 正文的摘要可跨运行复核，包含动态 artifact 或 invocation ID 的
canonical envelope 摘要只对这一组示例值有效。

```json
{
  "tool_name": "list_recent_builds",
  "tool_response_is_error": false,
  "tool_response": {
    "llmContent": "{\"builds\":[{\"id\":\"build-101\",...}],\"page\":1,\"page_size\":6}"
  },
  "execution_scope": {
    "environment_id": "env_33333333-3333-4333-8333-333333333333",
    "execution_context_id": "ctx_44444444-4444-4444-8444-444444444444",
    "actor_id": "act_55555555-5555-4555-8555-555555555555",
    "agent_session_id": "ags_11111111-1111-4111-8111-111111111111",
    "turn_id": "trn_22222222-2222-4222-8222-222222222222",
    "tool_use_id": "tol_66666666-6666-4666-8666-666666666666"
  }
}
```

COSH 先完成普通 PostToolUse Hook 的聚合。AW first-class effective-bytes boundary 随后读取
即将写入本地模型历史的 provisional bytes。这个位置避免 AW 处理早期 Hook 的脱敏副本，
最后却让另一份原文进入历史。

### Canonical input

AW Core 为 source bytes 分配 artifact identity。

```json
{
  "artifact": {
    "id": "art_9b44b763-ec58-c787-95f5-363ec02f80cb",
    "digest": "01202f4b809e4ffee777b3b7a62ca0d5007b99738c0abbc537fd1cc29d7422e1",
    "content": "{\"builds\":[...]}",
    "media_type": "application/json",
    "origin": "api_response",
    "tool_name": "list_recent_builds"
  },
  "boundary": "post_tool",
  "constraints": {
    "allow_text_reencoding": true
  }
}
```

`CapabilityInvocation` 在 input body 之外另外携带 execution scope。上述 canonical input
body 为 1115 bytes，SHA-256 为
`9287a0290c71198b722b5820c9266610ec86ec7fd573999c385a67866c6c4510`。

Source digest 与 canonical input digest 的作用不同。

| digest | 覆盖范围 | 作用 |
| --- | --- | --- |
| `0120...22e1` | 693-byte `llmContent` | 绑定被压缩的准确 source bytes |
| `9287...4510` | 完整 canonical input | 绑定正文、artifact metadata、boundary 与约束 |

Invocation scope 不属于这个 input body digest。Receipt 用独立 typed 字段保存并校验 scope。

Provider Host 在 mapping 前校验 canonical input Schema。

### Native request

Host 根据 Tokenless manifest 生成 native request，再按 Tokenless native Schema 校验。

```json
{
  "protocol_version": 1,
  "content": "{\"builds\":[...]}",
  "input_media_type": "application/json",
  "agent_id": "aw-provider",
  "session_id": "ags_11111111-1111-4111-8111-111111111111",
  "tool_use_id": "tol_66666666-6666-4666-8666-666666666666",
  "tool_name": "list_recent_builds",
  "seam": "post_tool",
  "content_origin": "api_response",
  "capabilities": {
    "replace_output": true,
    "publish_retrieve_tool": false,
    "replace_with_text": true
  }
}
```

`agent_id` 使用 manifest 中已准入的常量 `aw-provider`，表示通过 AW Provider 接入。
AW `environment_id` 继续保存在 invocation scope 与 receipt 中，不再伪装成 Tokenless
frontend identity。未来如果要按真实前端统计，需要新增独立的 typed 字段。

### Native response

Tokenless 对这份真实 fixture 返回下列稳定结果。

```json
{
  "protocol_version": 1,
  "disposition": "applied",
  "output": "builds[6]{id,project,status,duration_ms,owner}: ...",
  "output_media_type": "text/plain",
  "content_type": "json_records",
  "compressor_chain": ["toon"],
  "reversibility": "lossless",
  "before_tokens": 174,
  "after_tokens": 110,
  "stash_keys": []
}
```

完整 output 为 438 bytes，保留六条构建记录、`page` 和 `page_size`。Host 校验 native
response 后，再把 `output_media_type` 映射到 `candidate.media_type` 并校验 canonical
output。

### Candidate 与 receipt 返回 Core

Host 生成的 transient candidate 主要字段如下。

```json
{
  "source_artifact_id": "art_9b44b763-ec58-c787-95f5-363ec02f80cb",
  "source_digest": "01202f4b809e4ffee777b3b7a62ca0d5007b99738c0abbc537fd1cc29d7422e1",
  "content": "builds[6]{id,project,status,duration_ms,owner}: ...",
  "media_type": "text/plain",
  "transform_chain": ["toon"],
  "reversibility": "lossless"
}
```

Bare candidate object 的 canonical digest 为
`32b602230eaa68778419f4b3598b6402abd4365b62a0056ed8121bb23f4999a1`。真正由
`ProviderPayload.body`、Receipt 和 Ledger 引用的是 `{"candidate": ...}` envelope，大小为
737 bytes，digest 为
`586a7bdfbef6b99c4d132cefa3c83b131716ac81e7f21a3204d4e80a93b1d890`。Candidate content digest
为 `6c847696df69b21a2997cf599d6caf2bb5af76f418869c16cf07c0dc7e2d3003`。

Receipt 同时返回 Core，主要内容如下。

```json
{
  "provider_id": "tokenless",
  "capability": "context.projection.prepare/v1",
  "input_schema": "context.projection.prepare.input/v1",
  "input_digest": "9287a0290c71198b722b5820c9266610ec86ec7fd573999c385a67866c6c4510",
  "output_schema": "context.projection.prepare.output/v1",
  "output_digest": "586a7bdfbef6b99c4d132cefa3c83b131716ac81e7f21a3204d4e80a93b1d890",
  "disposition": "produced",
  "meters": {
    "source_tokens": 174,
    "prepared_tokens": 110
  }
}
```

这是便于阅读的字段投影。真实 Receipt 使用 typed capability 和 schema identity，并包含
manifest digest、scope、长度、时间与 invocation identity。它不保存 candidate content。

### COSH final adoption

Core 检查 candidate 的 source identity、source digest、媒体约束和合同状态。COSH 再根据
“非空且严格 `lossless`”的本地政策选择 candidate 或保留 source，随后写入本地
模型历史槽位。写入完成后，Ledger 记录
`aw.ledger.context_adoption/v1`。

```json
{
  "source_artifact_id": "art_9b44b763-ec58-c787-95f5-363ec02f80cb",
  "source_digest": "01202f4b809e4ffee777b3b7a62ca0d5007b99738c0abbc537fd1cc29d7422e1",
  "candidate_envelope_digest": "586a7bdfbef6b99c4d132cefa3c83b131716ac81e7f21a3204d4e80a93b1d890",
  "effective_digest": "6c847696df69b21a2997cf599d6caf2bb5af76f418869c16cf07c0dc7e2d3003",
  "effective_byte_count": 438,
  "decision": "adopted",
  "reason": "lossless_candidate"
}
```

Core 结果返回后先形成 `post_tool_use_plan`。Plan 中 content/code 两个 Observe step 必须
分别得到 observation 或明确 gap；同一步不能既有 observation 又有 gap。COSH 写入历史后，
上面的 adoption 才能引用已存在的 plan 和完全相同的 invocation references。Invocation
reference 还要与 Ledger header 的 attempt、tool use scope 一致。两条记录都不保存 source
或 candidate 正文。

### 禁止文本重编码的结构化路径

当 canonical input 设置 `allow_text_reencoding=false` 时，manifest 仍传递
`input_media_type=application/json`，但把 native `replace_with_text` 设为 `false`。Tokenless
不会选择 Toon，返回的 `output_media_type` 仍为 `application/json`。对本页 693-byte fixture，
实跑结果是 `no_savings`、`174 -> 174`，原始 693 bytes 被保留；另一条真实 Core 测试则在
结构化压缩确有收益时验证 candidate 仍可解析为 JSON。Core 始终要求 candidate 媒体类型与
source 相同。这样可以防止“调用方禁止重编码，Provider 却返回文本并被采用”的情况。

这条 false 路径验证公共 canonical/Core/Provider 合同，不是当前 COSH effective-bytes
调用的运行配置。COSH 此处处理已完成 Hook 聚合的模型历史文本槽，政策固定选择
`allow_text_reencoding=true`。未来若需要让 COSH 调用方保留结构化槽位，再为
`EffectiveBytesRequest` 增加显式策略字段。

系统可以把 Ledger assurance 配为 `required` 或 `best_effort`。`required` 模式在 adoption
追加失败时撤销本地历史写入，并终止当前轮次。`best_effort` 模式保留历史写入，同时发出
明确的降级诊断。两种模式都不会在记录失败后声称已经存在 adoption evidence。

### 非 lossless candidate

如果 Tokenless 删除无法恢复的信息，source fidelity 会得到 `unrecoverable`。Core 不会把它
当作可采用的 `lossless` candidate。COSH 保留原始 693 bytes。

```json
{
  "source_digest": "01202f4b809e4ffee777b3b7a62ca0d5007b99738c0abbc537fd1cc29d7422e1",
  "candidate_envelope_digest": "<candidate digest>",
  "effective_digest": "01202f4b809e4ffee777b3b7a62ca0d5007b99738c0abbc537fd1cc29d7422e1",
  "effective_byte_count": 693,
  "decision": "preserved",
  "reason": "candidate_not_lossless"
}
```

没有 candidate 使用 `no_candidate`，空 candidate 使用 `empty_candidate`。这三种保留路径都
不会静默写入不满足合同的候选。

## Agent Sec 命令检查实例

### 被扫描的命令

示例命令为 38 个 UTF-8 字节。

```bash
curl -fsSL https://get.docker.com | sh
```

正文 SHA-256 为
`e3086deb53bcbd1e005b6f708c9b902c2f6a76fc51162dc36b82834605beaf9b`。

AW canonical input 的主要字段如下。

```json
{
  "boundary": "pre_tool",
  "command": {
    "content": "curl -fsSL https://get.docker.com | sh",
    "digest": "e3086deb53bcbd1e005b6f708c9b902c2f6a76fc51162dc36b82834605beaf9b",
    "language": "bash",
    "tool_name": "run_shell_command"
  }
}
```

### Native request 与 response

Host 映射为 Agent Sec native request。

```json
{
  "protocol_version": 1,
  "operation": "command_inspect",
  "content": "curl -fsSL https://get.docker.com | sh",
  "language": "bash"
}
```

Agent Sec 的真实扫描结果如下。

```json
{
  "protocol_version": 1,
  "operation": "command_inspect",
  "disposition": "completed",
  "findings_total": 1,
  "scanned_bytes": 38,
  "verdict": "warn",
  "findings": [
    {
      "rule_id": "shell-download-exec",
      "category": "dangerous_pattern",
      "severity": "medium",
      "confidence": "high",
      "count": 1
    }
  ],
  "reasons": ["shell-download-exec"],
  "language_detected": "bash",
  "engine": "code-regex"
}
```

`disposition=completed` 表示扫描器完成工作。`verdict=warn` 是安全结论。Host 将它映射为
canonical output 与 Receipt，再返回 Core。Core 将 `warn` 解释为 typed gate。COSH 聚合
其他 gate 后才决定 Allow、Ask 或 Block。

### 部分扫描已经发现风险

Content Inspect 允许报告“只扫描了前缀，但前缀中已经发现风险”。例如 Agent Sec 的真实
PII scanner 对下列输入应用 25-byte 上限时，完整输入超过上限，而前 25 bytes 已包含一个
电话号码规则命中。

当前 AW content-inspect request 没有开放 `max_bytes` 字段，默认 Provider 路径会扫描完整
输入；这个例子用于说明并验证 canonical partial-risk 合同，而不是声明调用方已有可配置
截断开关。

```text
input              联系电话 13800138000 后续内容继续
scanned_bytes      25
truncated          true
```

对应 canonical inspection 可以写成下面的形式。

```json
{
  "inspection": {
    "verdict": "suspicious",
    "findings": [
      {
        "rule_id": "phone_cn",
        "category": "personal_data",
        "severity": "medium",
        "confidence": "medium",
        "count": 1
      }
    ],
    "scanned_bytes": 25,
    "truncated": true
  }
}
```

这个结果只证明已扫描前缀存在风险，不证明未扫描尾部安全。Core 接受截断事实的条件是
`0 < scanned_bytes < input_bytes` 且 verdict 带有 finding。若 25-byte 边界切在一个中文
字符中间，scanner 会回退到最后一个完整 UTF-8 字符，`scanned_bytes` 只记录真正解码并
扫描的字节数。`clean + truncated=true` 会被 Schema 和 typed validator 拒绝。

### 尚未闭合的执行字节绑定

Receipt 现在可以证明 Agent Sec 扫描了 digest 为 `e308...af9b` 的输入。COSH 仍可能在安全
检查前后应用其他 Hook patch。PoC 尚未把最终 spawn 参数的摘要与 gate credential 比对。

目标不变量如下。

```text
digest(scanned_command) == digest(executed_command)
```

这项校验必须放在最后一次参数修改之后、工具进程启动之前。任何后续修改都应使旧凭据
失效。当前安全示例证明 Provider、Host 和 Core 的往返链路，不能证明完整执行门禁闭环。

## Checkpoint State Provider 实例

### 为什么不能使用 one-shot Provider

Checkpoint 创建会修改持久状态。请求发送后连接中断，快照可能已经创建。自动重试会导致
重复副作用。原 PR 没有 Checkpoint Capability、Schema、manifest 或 Plan step。

fork PoC 将它放入 Gateway State Provider。Gateway 已经拥有 Task、Run、Attempt、批准与
durable ledger，适合保存有状态操作的生命周期。

PoC 在 `workspace-checkpoint-v1` profile 下向 COSH 注册
`workspace_checkpoint_create`。COSH 不直接连接 ws-ckpt，而是发送私有 control request；
Gateway 把 tool name、arguments digest、tool use 与当前 Task、Run、Attempt 组合成受治理
操作。未启用该 profile 时，工具不会出现在 Runtime tool registry 中。

### Gateway binding

Gateway 在批准前读取并持久化受信任 binding。下列对象是便于阅读的逻辑投影。

```json
{
  "profile": "workspace-checkpoint-v1",
  "provider": "ws-ckpt",
  "socket": {
    "path": "/run/ws-ckpt-agent-work/ws-ckpt.sock",
    "device": 30,
    "inode": 23361,
    "daemon_uid": 0
  },
  "workspace": {
    "ws_id": "ws-db447c",
    "registered_path": "/var/lib/anolisa-agent-work/workspaces/interactive-agent",
    "pinned_inode": 41728,
    "generation": "<Btrfs FSID + subvolume UUID>"
  },
  "gateway_uid": 1000,
  "permit_id": "prm_<stable id>",
  "execution_id": "exe_<stable id>"
}
```

`daemon_uid=0` 表示 ws-ckpt 服务端。`gateway_uid=1000` 表示 Gateway 调用方。两者不能使用
同一个字段。Gateway 还会把 profile manifest digest、target 和 binding 一起计算
`target_identity_digest`。

Binding 从 Runtime 已经 pin 的 directory file descriptor 读取 workspace inode 和 Btrfs
FSID/subvolume UUID，不再关闭句柄后按字符串路径重新打开。Approval binding v4 在 plan
阶段预先保存本次操作唯一的 `permit_id` 与 `execution_id`。批准后 permit、claim、start、
terminal、source 和 delivery 全部复用这两个 ID，不能重新生成一组身份。已经签发的 permit
可以在期限后按相同 Ledger receipt 重放；过期后不得签发新 permit。

### Guarded Checkpoint V2

批准与 durable claim/start 完成后，Gateway 调用 ws-ckpt。实际 wire 使用 bincode，下面用
JSON 表示字段。

```json
{
  "GuardedCheckpointV2": {
    "ws_id": "ws-db447c",
    "registered_path": "/var/lib/anolisa-agent-work/workspaces/interactive-agent",
    "expected_generation": "<opaque 32 bytes>",
    "checkpoint_id": "ckp_<stable id>",
    "operation_digest": "<32-byte digest>",
    "message": "COSH governed Task checkpoint",
    "pin": false
  }
}
```

ws-ckpt 校验 peer credential、workspace identity 与 generation，随后创建或跳过快照，并把
相同 operation digest 写入 durable evidence。Gateway 把结果收敛为 `created`、`skipped`、
`denied`、`failed` 或 `uncertain`。

ws-ckpt 的 Guarded V2 是新增 wire variant，既有 V1 variant 的顺序和字段保持不变。Gateway
控制协议也保留原 `cosh.gateway.v1` Submit JSON shape，但服务端只在 exact
`task-only-v1` profile 下接受 v1。Checkpoint 使用当前 v2 client，先做 Admission discovery，
再在 Submit 中回显完整 admission。Checkpoint 使用 v1、v1 携带 v2 echo、v2 缺少
echo，或用 v1 发送 Admission，都会被明确拒绝。

### 断连后的恢复

Gateway 只调用 `CheckpointEvidenceV2` 查询同一个 checkpoint ID 与 operation digest。
恢复时仍会验证当前 socket 位于受信任祖先目录、owner 正确，并通过 `SO_PEERCRED` 核对
daemon UID；它不会要求当前 socket inode 或 workspace path inode 与历史值相同，因为服务
重启可以合法更换这些运行时对象。

| evidence 状态 | Gateway 结果 | 是否再次 create |
| --- | --- | --- |
| 精确匹配且 outcome 为 created | 恢复为 succeeded | 否 |
| 精确匹配且 outcome 为 skipped | 恢复为 succeeded / skipped | 否 |
| evidence 缺失 | `uncertain` | 否 |
| digest、workspace 或 generation 不匹配 | `uncertain` | 否 |
| 当前受信任 socket endpoint 无法认证 | 拒绝恢复 | 否 |

这一规则保证系统不会用重试掩盖未知副作用。

### 当前边界

Checkpoint 当前属于 Gateway State Provider，不属于 AW manifest Provider。仓库不应增加一份
虚假的 `providers/ws-ckpt/provider.toml`。通用 AW State Provider 需要先定义 service
driver、binding lifecycle、reconcile 和 readiness。

PoC 已有 Runtime tool、Gateway codec、approval/execution 与 State Provider 代码路径，单元
和协议测试可以覆盖受控创建及返回字段。固定提交 `5ebfc0b3` 的 `deploy-vm.sh` 已在真实
Btrfs subvolume 上通过 Herdr 三段 E2E。Checkpoint 链产生 approval、permit、execution、
snapshot、durable evidence 与 11 个连续 Task 事件，终态为 `task_succeeded`。这次正常链路
没有注入响应丢失或重启，不能替代 evidence-only 恢复演练，也不能作为生产可用证明。

### VM 实际终态

下面的字段来自同一轮 Herdr `plugin-log-11`，不是文档占位值。

| 字段 | 实际值 | 语义 |
| --- | --- | --- |
| Task | `tsk_2b2519ae-d4e8-48ba-8a8d-f8888954b215` | 本次受治理任务 |
| Run | `run_eef93338-b09a-4174-aa73-4167594d4a12` | Task 的这一次执行 |
| Approval | `apr_7549cf4e-5447-49dd-8806-00fa8fa5ab51` | 用户批准记录 |
| Permit | `prm_9c1b82f2-345c-494e-95f6-670ddfe0766f` | 单次执行许可 |
| Execution | `exe_3f3ad3cd-4d60-4bab-8092-de9c966b24bb` | 副作用执行身份 |
| Snapshot | `ckp_8e84400b-7d29-4fc8-947b-e6a8a537ec20` | ws-ckpt 创建的真实快照 |
| Evidence ref | `268a2962...68812a` | Gateway 保存的 durable evidence 引用 |
| Operation digest | `43c58469...9c842a2` | 请求副作用的稳定摘要 |
| Target digest | `c765d874...3ad1162` | profile、target 与 binding 的摘要 |
| Task terminal | `task_succeeded` | 11 个连续事件后的终态 |

这组身份不能互相替代。Approval 证明谁批准了操作，Permit 证明本次许可，Execution 绑定
真正的副作用，Snapshot 是 ws-ckpt 的结果，Evidence 用于断连后查询。正常链路已经证明
这些字段可以贯穿一次调用；恢复链还要通过响应丢失和进程重启测试证明只查询、不重放。

## 开发验收摘要

| 链路 | PoC 已证明 | 仍待证明 |
| --- | --- | --- |
| Tokenless | 真实 Provider 往返、严格 source fidelity、COSH 本地采用、typed Ledger | 产品安装、长期健康、远端模型消费 |
| Agent Sec | 真实扫描、输入绑定、typed gate 解释 | 最终 executed-bytes binding |
| Checkpoint | Runtime tool、Gateway State Provider、Guarded V2、query-only reconcile 与 Ubuntu VM + Herdr 正常链路 | response-loss/crash-restart 故障演练、通用 State Provider 抽象 |

本机已真实验证 Provider Host、Agent Sec、Tokenless 与 COSH final adoption，得到
`693B -> 438B`、一条 plan 和一条 adoption record。Ubuntu VM + Herdr 又在同一固定提交上
验证三段链路，包括真实 Provider trace、438B 最终采用与 governed checkpoint。构建、CI、
签名安装、长期运维和故障恢复继续属于后续交付验收。
