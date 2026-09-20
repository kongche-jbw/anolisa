# AW 工具结果检查与投影

[English](../../en/user-entrypoint/aw.md)

AW 让显式配置的 Qoder 或 Codex hook 通过 SecCore 检查工具输出，并保留可核验的执行记录。
`qoder`/`codex` 检查命令保持原始工具结果不变；额外显式 Qoder 投影模式可返回候选，
并独立记录 local_history 采用。检查用于观察：提示不代表执行批准，也不证明 Agent 展示或采纳了提示。

这是实验性的 Linux 开发者入口。AW 尚未注册 `anolisa install` 或发行 RPM。
在仓库根目录构建已有源码：

```bash
cargo build --manifest-path src/aw/Cargo.toml --locked -p aw-hook-cli
src/aw/target/debug/aw-hook-cli --help
```

## 调用方式

```text
aw-hook-cli <qoder|codex|qoder-project> /absolute/SETTINGS.json
```

Agent 通过 stdin 传入一个原生 `PostToolUse` JSON 对象，然后关闭管道。
输入和配置各有 1 MiB 编码上限；stdin 必须在五秒内结束。
重复 JSON 键（包括嵌套键）会被拒绝；原生 Unicode 键及有限数值元数据会保留。

只接受[原生 adapter profile](../../../../src/aw/docs/design/native-adapters_zh.md)
支持的文本字段。两个宿主均要求原生 `session_id` 和 `tool_use_id`。
Codex 还要求原生 `turn_id`。Qoder 要求 launcher 提供 `qoder_single_turn_id`，
并负责在一轮结束后终止该次调用；不支持跨交互轮次复用同一配置。

CLI 不安装 hooks、不创建 Agent 会话，也不替代现有 SecCore 安全插件。
接入 launcher 必须使用宿主支持的 hook 配置注册命令、绑定存活的 Agent 身份并保护配置。
手工构造 payload 的测试不证明 Agent hook 已安装或其响应已展示。

## 配置参考

配置必须是绝对路径的普通文件，不能是符号链接；所有者必须与 hook 用户相同，
且不能有 group/other 权限（通常为 `0600`）。未知字段会被拒绝。
父目录和 Journal 必须放在 Agent 无写权限的位置；仅文件权限不能隔离同用户 Agent。

| 字段 | 必需语义 |
| --- | --- |
| `runtime` | launcher 认证的 `runtime-binding/v1` 快照；`observation_source` 为 `owned_child`，`process_ref` 为 `pid:<agent_pid>@<agent_start_ticks>` |
| `scope` | 与 runtime 匹配的可信 AW scope；原生 session/tool/turn ID 可填充未设置字段，但不能覆盖冲突值 |
| `agent_pid` | 存活 Agent PID，必须是 hook 的祖先进程 |
| `agent_start_ticks` | Linux `/proc/<pid>/stat` 第 22 字段，用于核对当前进程代次 |
| `qoder_single_turn_id` | Qoder 必需的非空 launcher 单轮标识；Codex 可省略 |
| `journal` | 私有 Core `FileJournal` 的绝对目录 |
| `provider` | 下表规定的显式原生启动配置 |
| `include_low_confidence` | 布尔值；显式选择后才启用原生低置信度选项 |

`runtime` 和 `scope` 必须满足[已注册合同](../../../../src/aw/schemas/)。
[合同 fixture](../../../../src/aw/tests/fixtures/contracts.json) 可用于理解结构，
其中的合成身份不能复制为真实会话。库的 `process_identity(pid)` 为可信 launcher
提供 Linux 父 PID 和启动 ticks。祖先检查用于发现错绑，不会认证恶意进程提供的任意 runtime 声明。

| `provider` 字段 | 必需语义 |
| --- | --- |
| `provider_id`, `provider_version` | 操作者选择的 registry 身份和 Provider release |
| `program`, `program_sha256` | 绝对可执行路径及独立提供的预期小写 SHA-256 |
| `cwd` | 绝对原生工作目录；保留原生配置解析方式 |
| `args` | `--version` 或 `scan-pii` 操作之前的显式前缀参数 |
| `environment` | 完整环境键值；不隐式继承任何环境 |
| `pins` | 最多 32 个指定文件，各含绝对 `path` 及 `state`：`{"sha256":"<expected digest>"}` 或 `"absent"` |
| `limits.timeout_ms` | 原生调用预算，1–300000 ms；版本探测使用独立的调用预算 |
| `limits.input_bytes` | 精确原生 stdin 上限，1–4194304 bytes |
| `limits.output_bytes` | 原生 stdout 上限及本 hook 的 AW 输出预算，1–4194304 bytes |
| `limits.stderr_bytes` | 丢弃的原生 stderr 上限，1–4194304 bytes |

实际版本探测必须返回 `agent-sec-cli 0.12.0`。提供该原生安装所需的完整环境，
包括标准用户规则依赖的真实 `HOME`。AW 不选择另一个用户 home，也不静默忽略损坏的规则配置。
固定可执行程序，以及所声明安装相关的依赖和规则文件；显式 `"absent"` 可发现新增文件。
每个固定文件必须是至多 64 MiB 的普通文件。每次调用前后均复核指定文件。
这不等于完整解释器依赖认证，也不能防止执行期间的变更。主动更新版本或配置后需要重新准入。

## 检查结果、失败与退出

| 结果 | stdout / 退出码 |
| --- | --- |
| 已核验 clean 检查 | `{}` / `0` |
| 已核验 suspicious 或 sensitive 检查 | 通用 `systemMessage` / `0` |
| Provider、身份、输入、记录或检查失败 | 通用 unavailable `systemMessage`，stderr 有界诊断 / `1` |

响应不含原始工具文本、原生证据片段或 Provider stderr，
不返回替换 payload、allow/deny 决策或采用声明。
原生宿主决定如何展示消息或处理 hook 非零退出。

Core 在分发扫描前持久记录固定计划和 invocation，之后保存 receipt 与终态事实。
Journal 包含摘要和 receipt，不存原始检查输入或输出。失败或中断的事件仍保持占用，
不能自动重放。Core 占用事件之前的失败（包括版本探测失败）没有执行 receipt。

CLI 收到 SIGTERM/SIGINT 后请求取消。Host 停止后续启动、中断管道交换，
回收自己创建的进程组，并允许至多一秒额外清理核验。
外部 hook timeout 必须覆盖输入读取、版本探测、检查、清理和本地记录开销。
SIGKILL、离开受控进程组的后代及内核阻塞操作不在保证范围内。
这套进程处理不是 OS 安全沙箱。

禁用时，只移除所属 launcher 中的 AW hook 注册，保留已有原生安全插件。
需要防重复执行和检查历史时继续保留 Journal；删除 Journal 也会移除旧事件的这些保证。

取消、进程所有权和验证详情见 [Host 架构与接入](../../../../src/aw/docs/design/sec-core-host_zh.md)。

## 显式集成 smoke

常规 AW 门禁运行真实本地协议测试进程和 smoke 校验器自测，不要求 Agent 登录或原生 SecCore 依赖。
如需执行本仓库的实际扫描器，先准备其 frozen Python 3.11.6 安装，再显式传入解释器：

```bash
python3 src/aw/tests/native_smoke.py --host codex --mode payload --provider-python /absolute/venv/bin/python
python3 src/aw/tests/native_smoke.py --host codex --mode agent --provider-python /absolute/venv/bin/python
```

`payload` 构造 hook 输入，验证真实 Host/扫描器/Journal 后半链。
`agent` 运行所选真实 CLI；Codex 使用两次本地脚本化模型响应和 ephemeral 会话，不属于真实模型评测。
Qoder 使用其原生模型和隔离配置目录；缺少登录或 hook 未触发均返回非零。两种模式均不改用户 hook 配置。

Codex 默认 `--codex-sandbox read-only`。宿主无法启动沙箱时，可显式选择
`--codex-sandbox danger-full-access` 运行无沙箱 fixture；不会自动 fallback。
唯一脚本化工具命令打印合成凭据。该运行证明观察链和原始结果保留，不证明沙箱防护或执行批准。
脚本输出进程/端口归属与结果，核对原生审计、精确 hook 文本和 Journal，并删除临时目录。
它保留真实 `HOME`，但将用户规则查找指向临时的缺失文件，将审计和 telemetry 指向 fixture 目录。
这验证真实扫描链路，不覆盖操作者当前的用户规则集。

## Qoder 投影与历史观察

`qoder-project` 仅支持 launcher 绑定的单轮 Qoder、同步 `PostToolUse` hook，以及成功的
结构化 `Bash` 结果。检查模式继续支持裸字符串；投影模式不准入。声明的历史文件必须已存在、
属于 hook 用户，父路径由操作者管理。原生 `cwd` 须等于 `hook.provider.cwd`；若提供
`transcript_path`，须与 `history_path` 完全一致。

投影配置是独立对象，下列字段全部必填；原有检查配置不变，嵌套于 `hook`。

| 字段 | 含义 |
| --- | --- |
| `hook` | 上文各表中的完整检查配置 |
| `tokenless` | 与 `provider` 相同的原生启动配置；独立 ID、版本 `0.8.1`，实际探测返回 `tokenless 0.8.1` 的 Tokenless 可执行文件 |
| `record_directory` | 已有可信父目录下的绝对私有目录（`0700`）；原文/候选记录权限 `0600` |
| `history_path` | 准确绝对 Qoder JSONL 路径，普通文件、当前用户所有、末端非符号链接，最多 16 MiB |
| `history_profile` | 固定 `qoder-cli-1.1.47/jsonl-v1`，不自动探测版本 |
| `retention` | 固定 `source_and_candidate`，显式接受私有明文保留 |
| `max_observation_delay_ms` | 从投影 receipt 完成至实际观察的 1–300000 ms 窗口 |
| `accepted_reversibility` | 固定 `["unrecoverable"]`；原生 lossless 分类不等于 AW 恢复能力 |
| `allow_text_reencoding` | 显式布尔值，允许 Tokenless 文本重编码 |

`tokenless.environment` 必须显式设置 `TOKENLESS_STATS_ENABLED=0`、
`TOKENLESS_SLS_ENABLED=0`、`TOKENLESS_COMPRESSION_ENABLED=1`，不启用 recovery stash。
其他准入、限额和 pin 要求见 [Tokenless Host profile](../../../../src/aw/docs/design/tokenless-host_zh.md)。
外层配置文件遵循相同私有文件要求及 1 MiB 上限。每份 context/observation 另受
4 MiB canonical 文档上限约束，无法持久保留的 context 不交付。外部 hook 超时应覆盖两个版本探测、
两次串行调用、输入读取、回收与持久写入。

必需 SecCore 检查先于可选 Tokenless 投影。检查失败阻止投影调用；敏感发现仍是观察，不作 deny。
Provider 失败或无有效候选时不返回替换。已验证候选返回格式为
`{"hookSpecificOutput":{"hookEventName":"PostToolUse","updatedToolOutput":"candidate"}}`。
stdout 前持久化 context；无缓冲写入全部完成后写独立 marker，不留用户态待 flush 数据。
stdout 使用五秒非阻塞期限并响应取消，宿主停读也不会无限等待。持久化、交付或 marker 失败返回非零。
marker 只记录本地传输完成；原生解析、退出处理和后续 hooks 仍可能影响结果。
原生替换位置及组合规则见 [Qoder 官方 hook 参考](https://docs.qoder.com/cli/hooks)。

context 位于 `record_directory/<event_key>.json`；适用时产生同目录
`<event_key>.returned.json`、`<event_key>.observation.json`。
库返回的 `ProjectionResult.record_path` 标识 context 路径。上面的 package 构建同时生成两个 CLI：

```text
aw-hook-cli qoder-project /absolute/PROJECTION_SETTINGS.json
aw-adoption-cli observe /absolute/records/CONTEXT.json
aw-adoption-cli query /absolute/records/CONTEXT.json
```

Qoder 追加对应结果后、配置窗口到期前执行 `observe`。它仅读取声明文件，验证捕获时的 inode 和
前缀，并按 session、tool ID、cwd 和 input 匹配唯一有序调用/结果；结果文本必须在捕获后追加。
要求完整 JSONL 行，每行不超过原生输入的 1 MiB 上限。缺历史/结果或迟到时保持 unverified，
不显示收益数；损坏、重复、前缀变化或错绑定明确报错。不自动轮询。成功观察不可变；重复或中断
claim 是错误，不自动重放 Provider。

`query` 读取保留的 context、原始执行 Journal、可选交付 marker 及独立确认的 observation，
不创建文件、不运行 Provider、不重新打开 live history。输出仅含元数据：

| 字段 | 含义 |
| --- | --- |
| `execution_decision` | 完整 Core 结果，包括 preserve 或 cancelled |
| `prepared` | 已生成候选 envelope，不表示交付 |
| `returned` | 已有有效 stdout 完成 marker |
| `observation_status` | `unverified`、`adopted`、`preserved` 或 `overridden` |
| `proof_boundary`、`observation_kind` | 观察提交后为 `local_history` / `recorded_snapshot` |
| `saved_bytes` | 该快照的精确可归属字节差；已验证保留/覆盖为零，无证明为 null |
| `observation` | 已验证采用合同和证据引用，或 null |

只有完整 proceed 计划和匹配候选才判定 adopted。原文为 preserved；不同文本为后续变换，归属收益
为零。仅无候选不证明原文保留。缺 stdout marker 不否定独立历史事实，两种事实分开呈现。
字节差不是模型 Token 数、请求交付或计费节省；`query` 不重验后续历史变化。

所有记录与 Journal 的父路径都应处于 Agent 不可写位置。观察行及调用输入可能包含敏感明文，
查询输出和 Journal 仅含元数据。本机制不隔离能同时改写记录和证据的同用户进程。
按审计周期一起保留 context、observation、marker 和 Journal；不自动清理。禁用所属 hook 可停止新投影；
删除 context/observation 会失去查询证据；删除 Journal 才会同时丢失持久事件去重占用。启用实验性 profile 前须验证真实 Qoder 版本与 hook 安装；
私有 JSONL 格式不是官方稳定 API，常规门禁使用合成历史，不运行已登录 Agent。

## 依赖预检

在准备会话hook配置前，检查所选原生安装。上述源码构建同时生成 `aw-preflight-cli`：

```bash
src/aw/target/debug/aw-preflight-cli /absolute/PREFLIGHT.json
```

沿用相同私有文件要求与1 MiB上限。配置必须选择以下一种形式：

| `mode` | 必填字段 | 探测的原生版本 |
| --- | --- | --- |
| `inspect` | `security` | SecCore CLI 0.12.0 |
| `project` | `security`、`tokenless` | SecCore CLI 0.12.0，再探测Tokenless CLI 0.8.1 |

两个字段都使用前文的原生 `provider` 配置。Tokenless还必须设置投影参考中的三项原生控制。
Provider ID不能相同。未知字段报错；`inspect` 拒绝附带 `tokenless`，不静默忽略。
绝对程序路径、前缀参数和独立预期摘要由安装owner提供。保留Python虚拟环境的程序路径，
解析其软链接可能改变包metadata和导入环境。预检不搜索PATH、不自动安装、不切换备用版本。

预检先核对原生版本，再通过既有有界Host对固定公开文本 `AW startup protocol probe.`
实际调用SecCore scan-pii。原生规则、middleware和审计保持启用，**可能产生一条原生审计事件**。
任意完整且语义有效的结果均可通过，包括sensitive；缺命令、覆盖不完整、格式错误均失败。
普通hook构造不会为每个工具事件额外执行这次合成扫描。

退出0输出 `status: dependencies_ready`、mode、组件/版本及 `agent_started: false`、
`runtime_ready: false`。SecCore另带 `protocol_profile: agent-sec.scan-pii/v1`、
`protocol_probe: passed`、`probe_scope: synthetic_content`、`native_audit_possible: true`。
失败只输出静态组件诊断，不回显原生内容或环境。

SecCore产品V1/V2、原生协议、AW合同及制品版本分别管理。版本字符串不足以证明兼容：
Rust V2可能同版本但尚无PII命令。本次探针验证所选安装的实际调用面，不等于完整扫描器兼容
认证、daemon健康或最终阻断；仍需可信制品/配置pin和原生合同验收证据。不创建Journal、Agent
或采用事实。执行时重新检查，后续文件变化可使此快照失效。

### 可选的固定Herdr bundle

Herdr为可选展示组件，不是检查预检的前置依赖。获取未修改的Linux v0.9.0二进制及Apache-2.0
许可证时，选择父目录已存在的目标：

```bash
python3 -B src/aw/integrations/herdr/fetch.py /absolute/herdr-v0.9.0
```

内置[pin](../../../../src/aw/integrations/herdr/upstream.json)选择Linux aarch64/x86_64制品及
固定commit的许可证。每项下载最多128 MiB，CLI整体默认120秒（`--timeout` 大于0且不超过
600秒）。两项均通过校验后，使用Linux `renameat2(RENAME_NOREPLACE)` 发布完整目录。
命令输出二进制路径，不启动它或Agent。

完整、匹配且可执行的已有bundle离线复用，不改变内容或权限；摘要不匹配、不完整、软链接或
目标竞争均报错，不替换或修复用户文件。Linux缺少所需rename操作时明确失败。普通异常、
SIGTERM、SIGINT及超时会清理私有staging；SIGKILL或断电可能留下staging。发布后发生中断
或输出失败可能保留完整有效bundle，重跑同一命令可核验复用。安装不等于启用。
不再需要时仅删除显式选择的bundle；fetch不注册全局状态或hooks。

就绪JSON复用已有五秒有界、可取消的stdout交付；不消费输出的管道不会让预检无限等待。

## Qoder 单prompt会话

显式Linux启动器连接 **Qoder 1.1.47**、**cosh-shell 0.15.0 helper**及既有AW二进制。
编排使用Python3.11+，Provider可执行入口独立选择。在本源码目录运行：

```bash
python3 -B src/aw/integrations/qoder/session.py /absolute/LAUNCH.json
```

配置要求当前用户持有、0600普通文件、唯一JSON键；未知字段拒绝：

| 字段 | 内容 |
| --- | --- |
| `format` | `1` |
| `workspace` | 已有规范化绝对路径，仅ASCII字母/数字、`/`、`_`、`-`；限定已验证的原生历史路径编码 |
| `config_directory` | 已有Qoder原生配置根，认证仍由原生管理 |
| `session_directory` | 已有父目录下的新绝对路径；不覆盖已有目录 |
| `binaries` | 恰好包含qoder、cosh_shell、aw_hook、aw_preflight、aw_adoption；每项为绝对path及独立可信sha256 |
| `preflight` | 上述inspect/project配置；security.cwd必须等于workspace |
| `prompt` | 一次非空prompt，UTF-8最多32KiB |
| `timeout_seconds` | Agent阶段1–240秒整数；启动探针和清理另有有限预算 |
| `permission_mode` | 显式default、dont_ask或bypass_permissions；AW不授予最终执行权限 |
| `model` | 可选原生model名称/ID |
| `projection` | 仅project必填：retention=source_and_candidate、accepted_reversibility=[unrecoverable]及显式布尔allow_text_reencoding |

启动前核对版本/pin、原生配置/插件和合成安全协议调用。已有hooks、启用插件、disableAllHooks、
无法解析的配置、非默认QODER_CONFIG_DIR_NAME会明确拒绝；已禁用插件条目保持原样。
不关闭安全插件、不切换settings sources、不复制认证、不写全局hooks。附加配置只写入本次私有
目录；首个profile支持干净配置，活跃插件共存需要独立验收。

cosh-shell启动一个bootstrap子进程，子进程记录自身PID/start ticks后exec Qoder，保持身份。
Qoder只执行一次 `-p`，选择Bash内置工具，直接使用现有qoder/qoder-project入口。
本阶段不支持交互PTY、多轮/reset或多pane，不提前伪造历史文件；投影必须读取Qoder真实前缀。
Agent完成但没有投影记录时，没有采用证明。

退出后每个上下文只执行一次有限observe，最多128项，写入result.json。之后可以通过
`aw-adoption-cli query /absolute/SESSION/records/EVENT_KEY.json`重新核验已有快照。
adopted仅证明该次local_history记录，不证明模型消费或计费收益；缺证据保持unverified。

专用启动器持有本次cosh-shell子树，包括被接管的独立进程组Provider后代。取消、超时或根进程
提前退出都会清理；leader保留至最后一次组信号之后才回收，不从历史PID恢复停止权。
TERM宽限1秒、kill/reap最多3秒，无法确认退出则报错；启动工具stdout/stderr各有64KiB实时上限。
SIGKILL、主机故障或内核阻塞不能保证有序清理。

启动前失败删除本次新目录；一旦尝试启动，私有配置、Journal、记录和result/failure元数据保留，
用于诊断及显式保留策略。Qoder历史仍在所选原生配置根。保留期结束后只删除该次会话制品，
无需修改用户hooks。[设计与测试边界](../../../../src/aw/docs/design/qoder-session_zh.md)说明资源归属。

## 有界 Qoder 连续对话

复用同一组已准入的原生安装，顺序执行最多八个prompt：

```bash
python3 -B src/aw/integrations/qoder/conversation.py /absolute/CONVERSATION.json
```

输入须为绝对路径、调用方所有的0600 JSON文件，上限1MiB。字段如下：

| 字段 | 必填值 |
| --- | --- |
| `format` | 整数 `1` |
| `launch` | 上述单prompt启动配置，删除 `prompt`；`session_directory` 指向新的conversation根目录 |
| `turns` | 按顺序排列的1～8个对象，每个包含 `prompt` 和可选布尔值 `reset` |

例如 `turns` 数组可以是：

```json
[
  {"prompt": "Read the project summary."},
  {"prompt": "Explain the previous summary."},
  {"prompt": "Start a separate conversation.", "reset": true}
]
```

创建根目录前先验证全部prompt和配置。每轮使用相同的显式permission mode、原生配置、pin、
Provider和投影opt-in。已有根目录会被拒绝；重复调用不会覆盖证据或恢复中断的runner。

第一轮使用新的原生UUID，后续轮通过Qoder `--resume` 续接该精确UUID；`reset: true` 则创建
另一个新UUID。不接受任意外部会话、“最近会话”选择或fork。续接前要求本次会话有非空、普通
文件形式的完整JSON对象历史，上限16MiB。这只检查历史可读取及格式完整，不证明模型上下文
已经恢复，也不替代AW独立history reader与身份核验。

每轮都经原有cosh-shell helper启动新的Agent进程，使用不同turn ID和递增runtime generation。
整个序列共享一个进程owner及取消状态；只有上一轮Agent进程树已回收、观察操作完成，才启动
下一轮。非零退出、准备/观察错误、超时或取消都会停止序列，不自动重试。`timeout_seconds`
仍是每轮Agent预算，另加每轮已有的有界启动、清理和观察操作。

新根目录为0700，保留含prompt的 `conversation.json`、仅元数据的 `summary.json`，以及沿用
单prompt私有布局的 `turn-0001`、`turn-0002` 等目录。summary的 `completed` 表示所请求进程
及观察操作成功结束，不表示所有输出都投影或采用；具体事实看逐轮 `result.json` 和既有query
证据。停止时保留已尝试轮次的证据及诊断；reset不删除旧记录，也不把旧统计移入新会话。
即使第一轮准备失败，根目录的诊断仍保留。

这是顺序print-mode调用，不是常驻交互Agent、PTY、进程内 `/clear`、pane管理或Herdr UI。
独立调用拥有不同根目录与会话；清理不从持久PID恢复终止权限。原生历史按Qoder自身策略保留，
AW记录由操作方在审计保留期结束后删除精确归属目录。
见[连续对话归属设计](../../../../src/aw/docs/design/qoder-session_zh.md#有界连续对话)。

## 实验性自然 Qoder 入口

此 opt-in Linux/Bash profile 保留 Qoder 原生交互循环与 PTY，属于实验性观察切片，
不代表强安全模式或四 Agent POC 完成。使用 Rust 1.97.1 构建包含 AW 的产品：

```bash
cargo +1.97.1 build --manifest-path src/cosh-ng/Cargo.toml --locked -p cosh-shell --features aw
```

准备调用者持有、权限为 `0600` 的私有 JSON 配置，放在不可信项目内容之外。
先审阅 handler，再独立固定解释器和脚本摘要。下面的路径须替换为本机绝对路径，
摘要须替换为对应已审阅文件的 lowercase SHA-256。handler 工作目录必须等于启动 Qoder 的目录。

```json
{
  "format": 1,
  "required_safety": false,
  "qoder": {
    "program": "/absolute/path/to/qodercli",
    "program_sha256": "<reviewed Qoder 1.1.47 executable digest>"
  },
  "native_config_directory": "/absolute/path/to/native-qoder-config",
  "handler": {
    "provider_id": "tool-observer",
    "provider_version": "1",
    "program": "/usr/bin/python3",
    "program_sha256": "<reviewed interpreter digest>",
    "cwd": "/absolute/path/to/workspace",
    "args": ["/absolute/path/to/tool-observer.py"],
    "environment": {},
    "pins": [{
      "path": "/absolute/path/to/tool-observer.py",
      "state": {"sha256": "<reviewed handler digest>"}
    }],
    "limits": {
      "timeout_ms": 1000,
      "input_bytes": 65536,
      "output_bytes": 1024,
      "stderr_bytes": 1024
    }
  }
}
```

[示例 observer](../../../../src/aw/examples/tool-observer.py) 从 stdin 接收一个 AW JSON
事件，返回严格的 `{"format":1,"observed":true}`。输入包含 runtime、session、attachment、
tool、配置 revision 与原生结果状态，不含工具参数或结果原文。其他输出、pin 变化、非零退出
或超时均记录为可选观察失败，原生执行继续。此 executable 协议不提供沙箱。

在工作目录中显式启用已审阅的配置版本，再打开 cosh：

```bash
export COSH_AW_CONFIG=/absolute/path/to/aw.json
export COSH_AW_CONFIG_SHA256='<reviewed configuration digest>'
export COSH_SHELL_INTEGRATION=enhanced
/absolute/path/to/cosh-shell --shell bash
```

在终端中输入 `qoder`。作用域 shim 在 exec 前加入原生配置，不改全局 PATH、可执行文件、
登录状态或原有 Hook 文件。该 profile 接受位置参数 prompt、`--model`、`--name`，以及带
明确 ID 的 `--resume`；其他选项拒绝。范围限于直接前台启动；alias/function、
原生绝对路径、嵌套启动、管道、重定向与远程命令不属于已认证入口。shim 可以绕过，不提供
OS enforcement。`required_safety: true` 在启动 Agent 前拒绝。

reset 依据原生 session 身份切换。旧回调/完成结果不计入新 attachment；reset 不产生新原生
session ID、或同进程返回已退休会话时报告缺口。实验上限为128个 attachment 代次、每个
runtime 1024次工具调用和有界的 shell 启动次数，达到限制后开启新 cosh 会话。Qoder 历史与
权限设置保留原生语义。

当 Herdr 将 cosh 作为 pane 的默认 shell，并提供 `HERDR_SOCKET_PATH` 与 `HERDR_PANE_ID`
时，可选 worker 发布观察计数。将 [interactive.toml](../../../../src/aw/integrations/herdr/interactive.toml)
中的 rows 合并到已有 Herdr v0.9.0 配置。每个 pane 核对自己的 cosh PID；viewer 无权执行
handler 或控制 Agent。RPC 失败后三秒内 metadata 过期，私有 query 显示 viewer 可用性。
worker 最多刷新24小时。关闭 Herdr 展示不停止 Agent。真实原生 Hook 共存及 Herdr 视觉
展示仍与合成协议/PTY 测试分别验收。

在新开 cosh 前 unset `COSH_AW_CONFIG` 和 `COSH_AW_CONFIG_SHA256` 即可禁用。
正常 shell 退出会 join 可选 viewer 并删除 AW 临时记录；SIGKILL 可能保留该 shell 的私有
临时目录，原生 Qoder 历史沿自身保留策略处理。既有07B/07C1显式启动器继续可用，默认
feature 与安全策略未改变。参见[归属与证据](../../../../src/aw/docs/design/interactive-observation_zh.md)。

### 公共生命周期通知配置

要在其他生命周期位置执行命令，显式选择格式 2。这是实验性通知 profile：命令会收到完整
原生 payload，使用前确认脚本可以读取提示词、参数和结果。固定实际 Qoder CLI 可执行文件，
不要固定分发脚本。下面的无操作命令确认会话开始；可替换为审阅过的命令。
启用环境变量与启动命令沿用上文。

```json
{
  "format": 2,
  "required_safety": false,
  "qoder": {
    "program": "/absolute/path/to/qodercli",
    "program_sha256": "<reviewed Qoder 1.1.47 executable digest>"
  },
  "native_config_directory": "/absolute/path/to/native-qoder-config",
  "cwd": "/absolute/path/to/workspace",
  "notifications": {
    "session.start": [
      {
        "provider_id": "lifecycle-notifier",
        "provider_version": "1",
        "program": "/usr/bin/python3",
        "program_sha256": "<reviewed interpreter digest>",
        "cwd": "/absolute/path/to/workspace",
        "args": [
          "-c",
          "import json,sys; event=json.load(sys.stdin); print(json.dumps({'format':1,'observed':True}))"
        ],
        "environment": {},
        "pins": [],
        "limits": {
          "timeout_ms": 1000,
          "input_bytes": 1048576,
          "output_bytes": 1024,
          "stderr_bytes": 1024
        }
      }
    ]
  }
}
```

目前接受的通知键为 `session.start`、`input.submit`、`tool.before`、`tool.after`、
`permission.request`、`compact.before`、`compact.after`、`subagent.start`、`subagent.stop`、
`turn.stop`、`session.end`；owner 来源另接受 `runtime.observed`、`runtime.exited`、
`coverage.changed`。原生工具成功和失败都映射为 tool.after，通过 native_event 与
原样保留的 payload 区分。turn_id 为 null 并带未知原因；输入到达不证明 Qoder 接纳新任务。

每个键配置 1–4 个顺序执行的命令，provider_id 互不重复，cwd 与顶层一致。
执行前核对整条路由的 pin；单命令最多一秒，链执行最多两秒，每个 runtime 最多接纳
1024 个回调。成功时精确返回 `{"format":1,"observed":true}`；其他输出或命令失败记录
缺口，原生执行保持原状。Journal 防止同一已领取 occurrence 重跑；没有稳定原生 ID 的
回调分配新的 arrival ID，因此重复提交的相同提示词仍是不同发生。

格式 2 不包含 handler；格式 1 不包含 cwd、notifications、tool_guard、input_response、stop_response 和 tool_response。未知事件、未接通来源及
required 安全要求在准入时拒绝。Bash 变换/guard 可通过下节的独立配置试验；其余来源与
真实效果仍需逐项验收，不能计为 16 项已完成。见 [16 事件矩阵](../../../../src/aw/docs/design/interactive-observation_zh.md#qoder-16-事件适配矩阵)
中的来源证据、缺口和下一动作。query 与可选 Herdr 只读命令计数，真实 Qoder TUI/Hook
完整共存仍待验收。


首次工作区信任流程中，Qoder 可能没有 SessionStart。格式 2 会从首个可信主 Agent 输入
绑定原生会话并继续通知，同时 query 显示 observation_gap=true、session_start_observed=false，
不会补造 session.start。真正 reset 仍要求 SessionStart 和新的会话身份；未知或已退休会话
不能通过普通输入重新接入。

## 实验性输入提交响应

需要在 Qoder 处理前拒绝输入或附加上下文时，将下面的 `input_response` 字段加入格式 2。
它显式授权一个响应命令；通知命令继续使用原有的确认合同。`required_safety` 保持 false，
`notifications` 可以为空对象，命令 cwd 必须与顶层一致。使用前替换为审阅过的路径和摘要。

```json
{
  "input_response": {
    "provider_id": "input-policy",
    "provider_version": "1",
    "program": "/usr/bin/python3",
    "program_sha256": "<reviewed interpreter digest>",
    "cwd": "/absolute/path/to/workspace",
    "args": [
      "-c",
      "import json,sys; request=json.load(sys.stdin); print(json.dumps({'format':1,'decision':'continue'}))"
    ],
    "environment": {},
    "pins": [],
    "limits": {
      "timeout_ms": 1000,
      "input_bytes": 1048576,
      "output_bytes": 65536,
      "stderr_bytes": 1024
    }
  }
}
```

命令收到 `{"format":1,"scope":"input.submit.respond","event":{...}}`。
event 是已经认证的生命周期信封，包含原生 payload.prompt，Turn 身份仍未知。
只接受下面三种返回：

- `{"format":1,"decision":"continue"}`：继续原生处理。
- `{"format":1,"decision":"continue","additional_context":"reviewed context"}`：
  在原始输入旁附加上下文。
- `{"format":1,"decision":"reject","reason":"input policy declined"}`：拒绝本次输入。

此处不提供提示词替换或许可批准。context/reason 必须非空、不含 NUL，且不超过 16384 个
UTF-8 字节。额外/混合字段、通知确认、重复 JSON 键、非零退出、pin 变化、超时和取消均拒绝，
不交付上下文。响应命令最多一秒，并与此前通知共享回调的两秒期限，不重新开始计时。
失败通知的输出不能成为决策，但通知耗时仍计入预算。

adapter 将上下文映射到 UserPromptSubmit.additionalContext，将拒绝映射为原生 decision=deny。
独立 helper 将输入解析/binding 错误映射为 exit 2。reset 或会话结束会扣留旧响应。
仅含元数据的 claim 位于 shell 私有 input-response-journal；query 的 input_response 显示
experimental_native_response，不声称已经采用，查询也不会重跑命令。

固定 Qoder 1.1.47 print 探针已实际采用返回上下文；策略拒绝、畸形返回、超时、取消及缺 binding
均停止输入处理。其他 Hook 的拒绝仍生效，两份 Hook 上下文均被采用。杀死 AW helper 后，
原生处理会在缺少其上下文时继续。这些仅是 native Hook 保证，不等于 final/protected，也不能
证明已经控制模型请求边界。自然 cosh TUI 也从原生 Stop 回调确认上下文标记，随后策略/畸形/超时三次输入未产生新 Stop，
Agent 和 shell 正常退出。排队/补充输入语义及其他插件组合仍待验证。

## 实验性主 Agent 停止响应

需要在主 Agent 停止时检查回答，可将 `stop_response` 加入格式 2，单独授权一个命令；
notifications 可为空。使用核实后的路径/pin，cwd 与工作区一致，required_safety 保持 false。

```json
{
  "stop_response": {
    "provider_id": "stop-policy",
    "provider_version": "1",
    "program": "/usr/bin/python3",
    "program_sha256": "<reviewed interpreter digest>",
    "cwd": "/absolute/path/to/workspace",
    "args": [
      "-c",
      "import json,sys; request=json.load(sys.stdin); print(json.dumps({'format':1,'decision':'allow_stop'}))"
    ],
    "environment": {},
    "pins": [],
    "limits": {
      "timeout_ms": 1000,
      "input_bytes": 1048576,
      "output_bytes": 65536,
      "stderr_bytes": 1024
    }
  }
}
```

命令收到 `{"format":1,"scope":"turn.stop.respond","event":{...}}`，包含原生停止 payload，
Turn 身份仍未知。输出只允许以下两种：

- `{"format":1,"decision":"allow_stop"}`：允许停止，不宣称任务成功。
- `{"format":1,"decision":"continue","reason":"complete the missing check"}`：
  经原生 decision=deny 请求继续工作。

reason 必须非空、无 NUL，且不超过 16384 个 UTF-8 字节。混合/原生字段、畸形输出、
非零退出、pin 变化、超时和取消均返回 continue=false，并携带检查不可用诊断；不会请求更多工作，
也不会记录检查通过。专用 --aw-stop 对畸形输入或缺 binding 使用相同失败语义；Stop 的
退出码 2 会请求模型继续，因此不能作为错误回退。此处不批准工具，也不替换回答。

原生 stop_hook_active=true 时跳过响应命令，使用同一诊断请求停止，限制递归检查，
包括其他 Hook 请求的继续执行。后续原生 Stop 的该字段为 false 时，是另一次发生，可执行检查。
此限制依赖原生字段，不依赖已证明的 Task 身份；重复检查不认证任务完成。通知路由仍独立运行，
不能提供响应决策，其耗时计入同一两秒期限；响应命令最多一秒。reset/会话结束阻止旧响应交付。

私有 stop-response-journal 只记录元数据及决策/跳过/失败类别，native_adoption=unconfirmed。
query 显示 stop_response 配置状态且不重跑命令。固定 Qoder 1.1.47 print 探针已验证
允许停止、继续理由被采用和重复检查停止；畸形返回、超时、协作取消和缺 binding 均没有
触发下一次回答。另一 Hook 请求继续时，AW allow_stop 保留该请求；AW continue=false
在两种已测声明顺序中均使其停止。这些案例不认证任意 Hook/插件组合。

自然 cosh TUI 在用户级 Hook 共存下验证连续两次输入，每次继续理由均被采用，Stop 标志
false → true；新输入将其重置为 false。畸形/超时检查没有触发下一次回答。TUI 显示了
不可用诊断，/exit 后收到 runtime.exited，随后 shell 回收。print 模式未显示该诊断，且
检查失败时仍退出 0。原生退出码和 allow_stop 都不证明任务成功，已显示回答不会撤回。
helper 被杀后仅留下 started、没有完成记录，原生输出仍正常结束。因此仍不认证
final/protected，required_safety 继续不可用。

## 运行与覆盖通知

在同一格式 2 notifications 映射中增加 runtime.observed、runtime.exited 或 coverage.changed，
命令形状与上文一致。配置任一项即启动 cosh owner worker，不要求 Herdr。
这些 envelope 的 source 为 runtime_owner、native_event 为 null；原生 Hook payload 不能选择该来源。

- runtime.observed 记录 exec 前的准入登记与 PID/start ticks；不证明原生 exec 成功或 Agent ready，
  此时 Session/Turn 未知。
- runtime.exited 来自 exec 前打开的 pidfd，报告根进程退出；不依赖 SessionEnd，不消费 Bash 的
  wait 状态。exit_status 为 null、descendants_reaped 为 false，任务成功与否未知。
- coverage.changed 携带 previous/current 采样快照，记录回调接入、会话代次、已观察缺口、
  通知交付和根退出。采样间的中间态可能合并。OS 覆盖仍为 not_attached；
  回调静默或侧栏断开不能证明保护失效。

启动前等待 owner 确认登记，最多五秒；pidfd 不可用或 owner 未确认时拒绝这个显式启用的启动。
worker 在有界通知之间每 100ms 采样，观察窗口为 24 小时；每 shell 最多登记 128 次，
每 runtime 最多 1024 次覆盖变化。通知仍限单命令一秒、整链两秒。失败不重试，终态交付失败
也保留为缺口。关闭 shell 时取消正在执行的命令并关闭所持 pidfd；不承诺 owner 崩溃恢复或
关闭后的补投。观察窗口到期后需打开新 cosh 会话。

query 新增 owner_observation 与 runtime_observer 快照，原生回调计数单独保留。
查询和 Herdr 重连不执行这些命令。通知 Journal 只保留元数据/摘要；运行快照随 shell 临时目录
清理，外部留存由可信 handler 负责。required_safety 仍不支持。

## 实验性 Bash 最终检查

格式 2 可以增加以下 `tool_guard` 字段；它与通知路由分开。顶层 `required_safety` 仍须为
false，`notifications` 可以为空对象。将示例路径和摘要替换为已核实制品，并显式配置 scanner
所需环境与策略文件 pin；scanner cwd 必须与顶层一致。当前固定 CLI 版本为 0.12.0。

```json
{
  "tool_guard": {
    "transforms": [],
    "scanner": {
      "provider_id": "sec-code",
      "provider_version": "0.12.0",
      "program": "/absolute/path/to/agent-sec-cli",
      "program_sha256": "REPLACE_WITH_EXECUTABLE_SHA256",
      "cwd": "/absolute/path/to/workspace",
      "args": [],
      "environment": {},
      "pins": [],
      "limits": {
        "timeout_ms": 2000,
        "input_bytes": 1048576,
        "output_bytes": 1048576,
        "stderr_bytes": 1024
      }
    }
  }
}
```

这个配置在 Qoder PreToolUse 的 Bash 调用中运行 `scan-code --language bash --mode regex`。
pass 且无 findings 才返回检查过的参数；warn、deny、失败、畸形响应、超时和取消均返回原生
deny。通过扫描不会自动批准工具，原有审批继续生效。其他工具不受这个 Bash profile 检查。

`transforms` 可配置 0–3 个同形状的命令配置，provider_id 与 scanner 及其他变换互不重复。
命令顺序接收 `{"format":1,"event":"tool.before","candidate":...}`；candidate 包含 tool_name、
tool_input 和 cwd。精确返回 `{"format":1,"command":"新的 Bash 命令"}`，只能修改 command，
其他原生参数保持原样。所有通知和变换完成后才检查最终命令；整条工具前路径共享两秒预算，
每个变换或通知仍不超过一秒。scanner 可配置最多 2000ms，供版本核对与扫描共用，
实际还受整链剩余时间限制，不重开期限；已有更短配置继续生效。当前变换和通知均串行，
独立只读动作并行尚未实现。

query 的 effect 为 `experimental_native_bash_guard`，tool_guard 为 `configured_not_certified`；
不表示发生过实际阻断。扫描内容通过原生 CLI 的 --code 参数传递，可能对同机进程查看者可见；
AW Journal 仅保存摘要和检查结果，sec-core 自身的审计留存仍由其配置决定。

真实 Qoder 1.1.47 TUI 与 sec-core 0.12.0 已验证变换后的命令审批、原生拒绝、策略拒绝
和变换失败；这些有界证据仍保留下述 final 限制。

Qoder 1.1.47 的隔离 print 探针已验证参数替换和 deny，但也确认其他 Hook 可以覆盖参数，
Hook 退出 1 或被 SIGKILL 时原命令仍执行。专用检查入口会将可返回的解析/绑定等错误映射为
阻断退出码 2；它不能控制自身被杀后的原生行为。详见
[原生验收边界](../../../../src/aw/docs/design/interactive-observation_zh.md#原生消费与故障边界)。

required_safety/final 继续拒绝准入；有界 TUI 案例不认证全部 Hook/插件组合及故障模式。已有格式 1 与
未配置 tool_guard、input_response、stop_response 和 tool_response 的格式 2 保持通知行为。它不提供 OS 隔离，也不保证正则检查发现所有危险命令。

## 实验性交互工具结果响应

需要投影主 Agent 已完成的 Bash 结果时，将 `tool_response` 加入格式 2，显式授权一个
结果处理命令；通知确认不能取得替换权限。下面是配置片段，需替换为可信可执行文件、
摘要和工作目录：

```json
{
  "tool_response": {
    "command": {
      "provider_id": "result-projector",
      "provider_version": "1",
      "program": "/absolute/path/to/projector",
      "program_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
      "cwd": "/absolute/workspace",
      "args": [],
      "environment": {},
      "pins": [],
      "limits": {
        "timeout_ms": 1000,
        "input_bytes": 1048576,
        "output_bytes": 131072,
        "stderr_bytes": 4096
      }
    },
    "accepted_reversibility": [
      "unrecoverable"
    ]
  }
}
```

命令收到 `format: 1`、`scope: "tool.after.respond"`、完整 `event`、
`source: {"text": "...", "digest": "..."}` 和 `accepted_reversibility: ["unrecoverable"]`。
摘要为原始 UTF-8 文本的 SHA-256；Turn 保持未知。只允许返回以下一种结果：

```json
{"format":1,"decision":"preserve"}
```

```json
{"format":1,"decision":"replace","source_digest":"<request.source.digest>","text":"replacement text"}
```

替换时复制请求的实际摘要。空文本、NUL、超过 65,536 UTF-8 字节的文本、外来摘要、
额外字段/原生控制字段、重复 JSON 键及命令失败均不能替换结果；编码后的响应还必须符合
配置的输出预算。通知和响应共享原始两秒回调期限，响应命令最多一秒。取消、超时和非法
响应保留原始结果，并返回诊断字段。required safety 仍不支持：这是可选投影，不是强制
脱敏，也不能撤销已经执行的工具。

只支持已有的 Bash completed stdout 对象：退出码 0、signal 为 null、未中断、非图片、
未设置预期无输出标志且 stderr 为空。其他工具及 PostToolUseFailure 仍只通知；不支持的
Bash 结果形状不执行投影命令。前后配对的工具结果只认领一次，包括失败结果，后续回调
不能重试投影；reset/退出阻止旧候选交付。Journal 仅留原文/候选摘要，不留正文；受信任
命令会收到原生内容，其自身留存由操作者控制。

响应使用 Qoder 的 `updatedToolOutput` 槽位。query 报告配置和实验性替换支持，不重跑命令。
本切片有合成命令进程测试，真实 Qoder 采用、原生展示和其他 Hook 的覆盖顺序仍待验证。
它不会自动调用 Tokenless/sec-core，不提供恢复或 final/protected 保证；原有单轮
`qoder-project` 保持独立的检查及历史记录合同。
