# AW Provider 端到端 PoC

[English](README.md)

AW Provider 端到端 PoC 是一套可执行的架构基线，用于验证 Provider 准入、安全检查、
Tokenless 投影、COSH 最终采用、Ledger 证据和受控工作区 Checkpoint。演示使用真实的
Agent Sec、Tokenless 和 Ubuntu 虚拟机中的 ws-ckpt daemon。确定性的 mock model 只负责
选择工具，不替代任何被测 Provider 或有状态组件。

本目录用于架构评审和演示，不是生产安装器。

## 1. 一张图理解整体架构

Provider 只有同时满足以下条件才真正生效。

1. 受信任的系统配置启用 AW 边界。
2. Provider Host 发现并准入 Provider 包及其 Schema。
3. AW Core 把 Canonical Capability 路由到已准入的 Provider。
4. Provider Host 把 canonical input 映射为组件的 native protocol。
5. Provider Host 把 native result 作为 candidate 和不含正文的 receipt 返回 AW Core。
6. AW Core 校验跨字段语义，并把已确定的结果返回 COSH。
7. COSH 选择写入模型历史的准确字节。
8. Ledger 先记录 plan，再记录最终采用决定。

```text
工具结果
    │ 准确的源字节
    ▼
AW Core ──canonical input──▶ Provider Host ──native request──▶ Provider
   ▲                              │                              │
   │ candidate + receipt          ◀──────── native response ────┘
   │
   └── 已校验结果 ──▶ COSH history ──▶ context_adoption ──▶ AW Ledger
```

返回箭头是合同的一部分。Provider 进程产生输出不等于结果已经生效。结果必须经过
Provider Host 和 AW Core 返回，COSH 才能采用，Ledger 才能如实记录最终采用。

Checkpoint 会产生持久副作用，不能像纯转换一样安全重试，因此使用独立的有状态流程。

```text
COSH tool call
    ▼
Gateway Task ──▶ 持久化审批 ──▶ permit + execution claim
    ▼
ws-ckpt Guarded V2 create ──▶ 持久化的准确 evidence ──▶ Task result
                                           ▲
                         I/O 结果不明时查询 evidence，绝不重放 create
```

## 2. 完整演示包含什么

Herdr 的 `run-complete-e2e` action 会依次执行三条相互独立且带超时的 trace。

| Trace | 真实组件 | 必须得到的证据 |
| --- | --- | --- |
| Canonical Provider 字段 | Provider Host、AW Core adapter、Agent Sec、Tokenless、AW Ledger | discovery、receipts、candidate、gate 和 plan records |
| COSH 最终采用 | CoshCore、Agent Sec、Tokenless、AW Core、AW Ledger | 模型历史中的准确 438 字节，以及一条 plan 和一条 adoption record |
| 受控 Checkpoint | COSH Gateway、CoshCore、ws-ckpt Guarded V2 | approval、execution、audit digest 和一个新 snapshot |

第一条 trace 展示详细字段，但只到 adapter 边界。第二条 trace 才能证明 COSH 把
Tokenless candidate 写入 history，然后写入最终采用证据。第三条 trace 证明有状态副作用
经过了受控执行，并校验恢复所需字段；它不注入响应丢失、进程故障或重启恢复。

## 3. Provider trace 使用的真实字段

### 3.1 命令安全检查

提交的命令正好是 38 个 UTF-8 字节。

```text
curl -fsSL https://get.docker.com | sh
```

SHA-256 是
`e3086deb53bcbd1e005b6f708c9b902c2f6a76fc51162dc36b82834605beaf9b`。
Agent Sec 必须报告 `security.scanned_bytes=38`。固定输入会产生一个 finding，AW Core
把 gate 确定为 `warn`。这只能证明提交的字节接受了检查，不能证明随后某个无关 shell
执行了同一组字节。

### 3.2 Tokenless 投影

`list_recent_builds` 源数据是 693 个 UTF-8 字节，SHA-256 是
`01202f4b809e4ffee777b3b7a62ca0d5007b99738c0abbc537fd1cc29d7422e1`。
稳定的 execution scope 包含以下字段。

```json
{
  "environment_id": "env_33333333-3333-4333-8333-333333333333",
  "execution_context_id": "ctx_44444444-4444-4444-8444-444444444444",
  "actor_id": "act_55555555-5555-4555-8555-555555555555",
  "agent_session_id": "ags_11111111-1111-4111-8111-111111111111",
  "turn_id": "trn_22222222-2222-4222-8222-222222222222",
  "tool_use_id": "tol_66666666-6666-4666-8666-666666666666"
}
```

Provider Host 把 canonical artifact 映射为 Tokenless native request。Native
`agent_id` 使用固定的前端身份 `aw-provider`，不能把 AW environment ID 填进这个字段。
下面只列出关键字段，为保持可读性，省略了 693 字节的 `content` 值。

```json
{
  "protocol_version": 1,
  "input_media_type": "application/json",
  "agent_id": "aw-provider",
  "session_id": "ags_11111111-1111-4111-8111-111111111111",
  "tool_use_id": "tol_66666666-6666-4666-8666-666666666666",
  "tool_name": "list_recent_builds",
  "seam": "post_tool",
  "capabilities": {
    "replace_output": true,
    "publish_retrieve_tool": false,
    "replace_with_text": true
  }
}
```

真实 Tokenless 结果报告 174 个 source tokens 和 110 个 prepared tokens。Lossless
candidate 是 438 字节，SHA-256 是
`6c847696df69b21a2997cf599d6caf2bb5af76f418869c16cf07c0dc7e2d3003`。
因为本次调用允许文本重编码，native response 声明
`output_media_type=text/plain`。

```json
{
  "disposition": "applied",
  "output_media_type": "text/plain",
  "before_tokens": 174,
  "after_tokens": 110,
  "reversibility": "lossless"
}
```

Trace 还会用 `replace_with_text=false` 重放同一份 source 作为对照。这个分支必须保持
原始 693 字节，返回 `disposition=no_savings` 与
`output_media_type=application/json`，并且 output 仍能解析为 JSON。两份 native
response 都会保留在本次 run evidence 中。
Provider Host 把 candidate 与 receipt 返回 AW Core。随后 CoshCore 测试确认模型历史的
tool result 槽位包含同一组 438 字节。

### 3.3 最终采用的 Ledger records

测试会把 Ledger 保存在当前 run 目录，并校验一条准确的双记录哈希链。

| Record | 含义 |
| --- | --- |
| `post_tool_use_plan/v1` | 修改 history 之前已经确定的 Provider observations、projection offer 和无正文 receipts |
| `context_adoption/v1` | COSH 向本地 history 槽位提交的 candidate digest 和 byte count |

Ledger 只保存 digest、封闭枚举、计数和调用身份，不保存 693 字节 source 或 438 字节
candidate 正文。

## 4. Checkpoint 为什么是 State Provider 边界

Checkpoint 创建可能已经完成，但调用方没有收到响应。此时盲目重试可能创建第二个
snapshot。PoC 复用现有 Gateway 与 ws-ckpt Guarded V2 合同，没有把一次性 Provider
进程 driver 假装成安全的有状态执行器。

Gateway 把操作绑定到已准入的 capability profile、target、原始 workspace registration
path、解析后的 workspace inode、ws-ckpt generation、caller UID、checkpoint ID 和
permit ID、execution ID、operation digest。执行前必须持久化 operator approval。响应
状态不明时，恢复流程使用同一组 binding 查询准确的 Guarded V2 evidence，绝不重放
create request。

演示强制要求以下事件顺序。

```text
approval_requested
  → approval_resolved
  → execution_planned
  → execution_result_recorded(succeeded)
  → task_succeeded
```

为保证每次演示都可重复，runner 会作为本地演示操作员，只批准自己创建的 Task 所产生的
唯一 approval ID。它证明 approval 的持久化顺序和标识绑定，不用于演示人工审批界面。

提交前，runner 会在已注册 workspace 的 `.aw-provider-poc/` 下创建一个唯一 marker，
用于区分本次 checkpoint。`finally` 路径会从 live workspace 删除该 marker；保留的副本
只存在于 snapshot 内。

演示还会比较 Task 前后的 ws-ckpt inventory，并要求只出现一个新 snapshot。这个
snapshot 必须使用 Gateway 管理的 `ckp_<uuid>` 标识、原始 registration path 和受控的
checkpoint message。成功执行事件还必须保留一个 64 字符的 receipt digest。
runner 会立即写入 `submission.json`，并在 poll 期间原子更新 `task-events.json`。即使运行
失败或超时，目录仍保留准确查询状态所需的 Task ID；脚本不会自动启动替代 checkpoint
Task。

Checkpoint 副作用完成与 Task 完成是两个独立事实。Checkpoint 可能已经持久化，
但后续必须成功的 AW adoption 步骤失败；此时 Task 可以失败，checkpoint 副作用和
Ledger 证据仍然存在。恢复时必须查询准确的 execution evidence，不能根据
`task_failed` 推断“checkpoint 没有产生”，更不能重新提交 `create`。

## 5. 本地验证 Provider

本地 trace 需要 Linux aarch64、已构建的 Tokenless binary，以及仓库使用的 Agent Sec
Python 环境。

```bash
cargo build --manifest-path src/aw/Cargo.toml \
  -p aw-provider-host -p aw-cosh-hook -p aw-ledger
cargo build --manifest-path src/tokenless/Cargo.toml -p tokenless-cli
poc/aw-provider-e2e/scripts/run-provider-demo.sh
```

再单独运行准确的 CoshCore adoption proof。

```bash
cargo test --manifest-path src/cosh-ng/Cargo.toml \
  -p cosh-core \
  core::tests::real_providers_commit_effective_history_and_adoption_evidence \
  -- --ignored --exact --nocapture
```

Checkpoint 验证需要 Ubuntu 虚拟机，因为 ws-ckpt 和 Btrfs workspace 只支持 Linux。

## 6. 部署到现有 Ubuntu 虚拟机

部署前工作树必须干净。这样 bundle、test executable 和安装后的不可变 release 才能绑定
到唯一 source commit。

`build-bundle.sh` 读取 Cargo 的 `compiler-artifact` 消息来选择 `cosh-core` 单元测试
可执行文件，不猜测带 hash 的文件名。脚本还会确认该文件确实包含准确的 final-adoption
test，并把文件 digest 和大小写入 `build-info.json`。archive 与 SHA-256 checksum 会一起
复制到虚拟机。

默认 VM helper 路径是 `/home/kongche/anolisa/ws/ubuntu-26.04-vm`，可以通过
`AW_POC_VM_ROOT` 修改。VM 与以下共享服务必须已经运行。

- `ws-ckpt-agent-work.service`
- 原有的非 PoC `cosh-gateway@ubuntu.service`
- Herdr
- 原有 Agent Sec Python runtime

执行一个命令即可部署并运行全部证据 trace。

```bash
poc/aw-provider-e2e/deploy-vm.sh
```

部署脚本绝不启动或停止 QEMU，也不重启原有 Gateway 或 ws-ckpt service。它只安装并
重启独立的 `cosh-gateway@aw-provider-poc.service`。该服务的 socket、database、audit
log 与 Ledger 都使用 PoC 专属路径。systemd 只在这个 service 的 mount namespace 内把
PoC `cosh-system.toml` 挂载为 `/etc/copilot-shell/config.toml`，共享 Gateway 不会继承 PoC
Provider 配置。同一个 namespace 只向 Gateway 暴露只读的共享 workspace 目录树，
checkpoint 修改只能通过已准入的 ws-ckpt socket 边界发生。
部署脚本在构建前要求两个共享 service 都处于 active 状态，完整演示结束后会再次
确认它们仍然处于 active 状态。

| 资源 | 路径 |
| --- | --- |
| 不可变 release | `/opt/anolisa-mvp/aw-provider-poc/releases/<commit>` |
| 当前 release 链接 | `/opt/anolisa-mvp/aw-provider-poc/current` |
| 独立 Gateway socket | `/run/anolisa-aw-provider-poc/gateway.sock` |
| Gateway database 与 AW Ledger | `/var/lib/anolisa-aw-provider-poc/` |
| 用户可见 evidence | `/home/anolisa/.local/state/aw-provider-poc/` |
| Herdr plugin | `anolisa.aw-provider-poc` |

每次 Provider、Gateway、VM 传输和 poll 操作都有明确超时。部署失败时会保留诊断证据，
只清理本次随机 staging 目录。

## 7. 通过 Herdr 运行

运行全部三条 trace。

```bash
sudo -iu anolisa herdr --session anolisa-agent plugin action invoke \
  run-complete-e2e --plugin anolisa.aw-provider-poc
```

打开精简、只读的 evidence pane。

```bash
sudo -iu anolisa herdr --session anolisa-agent plugin pane open \
  --plugin anolisa.aw-provider-poc \
  --entrypoint provider-trace \
  --focus
```

也可以单独运行以下 action。

- `run-provider-trace`
- `run-cosh-final-adoption`
- `run-governed-checkpoint`
- `verify-latest`

Pane 使用纯文本与简短字段说明，不使用遮挡箭头或数值的浮层标签。

## 8. 清理与回滚

清理脚本要求显式确认，绝不停止 QEMU 或任何共享 runtime。

删除 plugin、独立 Gateway 和 PoC releases，保留全部 evidence 和 snapshots。

```bash
poc/aw-provider-e2e/cleanup-vm.sh --yes
```

同时删除独立 Gateway database、AW Ledger、无状态 Provider 与 adoption summaries。
成功的 Checkpoint summary 会与保留的 snapshot 一起保留。具有完整终态的 failed 或
cancelled Task evidence 也会保留，脚本不由 Task 失败推断副作用是否发生。非终态、
事件不连续、身份漂移或不完整的 evidence 会使清理 fail closed。

```bash
poc/aw-provider-e2e/cleanup-vm.sh --yes --purge-evidence
```

只删除“成功 summary 与当前 inventory 精确匹配”的 checkpoint ID，并只移除这些
summary。failed、cancelled、已不存在或不匹配的 checkpoint evidence 会继续保留。删除第一个
snapshot 前，脚本会先 unlink plugin，并确认独立 Gateway 已停止。

```bash
poc/aw-provider-e2e/cleanup-vm.sh --yes --purge-checkpoints
```

Snapshot 按创建时间倒序删除，最多处理 100 个已记录 ID。清理前会读取 workspace
inventory，并跳过已经不存在的 ID，因此部分完成后可以安全重跑。如果 ws-ckpt 拒绝仍然
存在的 ID，清理立即停止，不会猜测其他 target。

## 9. 明确的能力边界

- 磁盘上存在 Provider 包不等于 Provider 已启用。
- Candidate 不等于 adoption。只有 CoshCore history 断言和随后写入的
  `context_adoption` record 能证明本地采用决定。
- Adoption record 证明本地 history 修改，不证明远端模型已经收到消息。
- Checkpoint 集成是一条具体的 Gateway State Provider。通用 manifest 驱动的 AW State
  Provider 仍需要 service lifecycle、reconcile 和 readiness 合同。
- Checkpoint trace 执行成功的受控 create 路径。它校验恢复 identity 与 evidence 字段，
  但不执行故障、进程重启或仅依赖 evidence 的 reconcile。
- Happy path 不演示 checkpoint 副作用已完成，但后续 AW adoption 确定之前失败
  的情况；这两个结果必须能够独立观测。
- PoC 使用带 checksum 的本地 bundle。生产环境还需要签名包、策略控制的激活、健康状态
  与回滚证据。
