# 交互观察的归属

[English](interactive-observation.md)

状态：实验性开发基线，逐事件真实验收尚未完成。当前切片面向 Linux、Bash、Qoder 1.1.47 与
Herdr v0.9.0 metadata 协议。合成原生 peer 测试不认证这些产品的完整运行时行为。

可选 `aw` feature 将既有 AW 库装配进 cosh-shell。Bash 加载启动文件后，将 shell 作用域
shim 目录放到 PATH 首位。`qoder` shim 核对可信配置、可执行文件、版本、cwd、前台 TTY
与祖先身份，再 exec 原生 Agent，不添加 print-mode 参数。内层 Bash 负责子进程和作业控制，
cosh 负责既有 Bash PTY；没有新增 AW supervisor 或 Python subreaper。Provider/handler
仍由既有有界 Host 管理进程组。固定 Qoder 制品超过 Host 的 64 MiB pin 上限，因此 Agent
独立采用 512 MiB 有界流式校验，不扩大 Host 原有的文件限制。

Core 保持库边界，各 Agent 保留原生执行循环。在旧配置格式 1 中，纯观察不进入 Core 执行计划，也不产生安全或
采用回执。实验性 `tool.result_observed` 包含 runtime、source epoch、原生 session/tool、
attachment 代次和配置 revision，不传工具输入/输出。handler 只能确认观察，不接受替换、
授权、取消或最终 dispatch 效果。

原生 SessionStart/SessionEnd 绑定 attachment。PreToolUse 固定工具调用身份，post-tool
在调用 handler 前一次性 claim，中断不重试。状态锁只覆盖短转换，不覆盖 handler 执行。
reset 后完成的旧调用留在旧 attachment。同进程返回已退休会话、或 reset 不产生新原生
session ID，均暂不支持并报告覆盖缺口。进程 incarnation、attachment 与配置 revision 分开。

query 只读私有证据，不执行 handler。Herdr 检查 pane 的 shell PID 是当前 cosh，再发布
有有效期的快照。worker 的 socket 操作有界，刷新最多24小时；退出时先取消并 join，再删除
shell 作用域目录。关闭侧栏不停止 Agent。快照可以存在刷新间隔与 TTL 范围内的延迟，携带
attachment 身份且不授权执行。观察到退出不证明任务成功或后代全部回收。

原生配置通过 `--settings` 合并，不改写已有文件。原生 Hook 仍可改变结果或抑制回调，因此
覆盖缺口与未收到原生回调必须可见。required 安全模式在准入时拒绝。同用户私有文件与 pin
只检测意外变更/错绑定，不提供 OS 隔离。SIGKILL 无法运行析构；正常 shell 退出清理 AW
临时证据，原生历史保留由 Qoder 负责。

验证：AW interactive target 覆盖信任、required 拒绝、迟到/重复回调、调用中 reset、
handler 超时回收及 Herdr 投影；cosh shell_host AW target 在真实 PTY 上使用合成 peer，
覆盖 reset、resize、退出、双 pane 隔离及清理。真实 Hook 共存的有界验收见下文；固定 Herdr UI 仍待验收。

## 公共生命周期通知（配置格式 2）

格式 2 将公共生命周期事件交给 `aw-core::Core::notify` 与既有有界进程 Host。
stdin 保留完整原生 payload，包括失败工具结果。私有 Journal 只存身份、payload 摘要及
命令结果，不存正文或命令 stdout。每个事件配置 1–4 个顺序执行的可选通知命令，精确返回
`{"format":1,"observed":true}`。控制形状输出视为确认失败，不转发为原生决策。
本增量完成通知路径；候选变换、guard 和采用仍是后续工作。

执行首个命令前检查整条路由的 pin。Core 通过 FileJournal 领取来源 occurrence，RAII
释放 writer，持久保留 reservation。链执行使用两秒单调时钟预算，单命令最多一秒。
取消/reset 阻止后续命令并回收活动进程组；失败/中断的 occurrence 不重试。
原生没有发生 ID 的重复输入或 Stop 回调分配不同 arrival ID，内容相同不能证明是重放。
工具成功/失败共享一次结果 occurrence。查询及 Herdr 刷新不执行命令。

原生登录准入失败可能先产生 SessionEnd 而没有 SessionStart；此时只建立终态观察。
未知 AW Turn 保持 null 并记录原因；子 Agent 生命周期保留原生 agent_id，不替代 runtime
身份。子 Agent 的工具/Stop 回调仍需独立子绑定，当前主运行 profile 拒绝这些回调。

真实 1.1.47 TUI 的首次工作区信任流程可能缺少 SessionStart，但后续回调正常发生。
格式 2 允许首个认证的主 Agent UserPromptSubmit 绑定尚未建立的会话，记录
observation_gap=true、session_start_observed=false，不合成 session.start。工具先到、子 Agent、
活动期间其他会话及 SessionEnd 后的输入都不能借此建立绑定。后续真实 SessionStart 仍按
正常 reset 规则处理并记录启动观察；Turn 保持未知。

## Qoder 16 事件适配矩阵

参考 CLI：**1.1.47**，实际可执行文件 SHA-256：
`7dfd7ff973ec64f3d3f23723b406e4c40387c9eb5accc0364ede29540ec77e45`。
应固定实际 `qodercli`，不固定 `qoder` 分发脚本。
[官方 CLI Hook 文档](https://docs.qoder.com/cli/hooks)与制品符号用于确定候选来源，不代替
完整运行认证。下表 N 表示通知接线与合成 payload/进程测试通过，不表示真实产品已逐项验收。
所有 N 路径只返回空原生响应或诊断 systemMessage；可写字段为空，无拒绝/final 权限。
其他原生 Hook 仍可能修改内容或抑制回调。

| 公共事件 | 来源 / 当前状态 | 证据或下一动作 |
| --- | --- | --- |
| session.start | SessionStart / N | 真实 compact/clear/resume 已观察；首次信任缺 startup |
| input.submit | UserPromptSubmit / N + 独立响应 | 下文已验证上下文/拒绝及原生 print 案例；排队/补充待验，Turn 未知 |
| tool.before | PreToolUse / N | 真实 Bash 参数已观察；native guard 证据见下文 |
| tool.after | PostToolUse / PostToolUseFailure / N + 显式响应 | 固定版 print/TUI 后续回答已采用 Bash 成功文本替换；已证实 peer 可覆盖，历史核验及其他结果形状待验收 |
| permission.request | PermissionRequest / N | 真实一次性批准/拒绝已观察；AW 不返回审批决策 |
| compact.before | PreCompact / N | 真实手动压缩已观察；自动压缩待验收 |
| compact.after | PostCompact / N | 真实手动完成已观察；失败覆盖待验收 |
| subagent.start | SubagentStart / N | 真实无工具子 Agent start 已观察，关联父 session/runtime |
| subagent.stop | SubagentStop / N | 真实同身份子 stop 已观察；重复停止检查待验收 |
| turn.stop | Stop / N + 独立响应 | 固定版 print/TUI 继续采用、重复标志及有界 Hook 共存已验证；不推断任务成功 |
| session.end | SessionEnd / N | 真实登录失败、clear 和正常 TUI 退出已观察 |
| model.before_request | 无已接通发送方 / 缺口 | CLI 目录/发送接线待核实；SDK 选择器无完整请求 |
| runtime.observed | cosh owner / N；合成 PTY 通过 | 真实 TUI 已观察 exec 前 pidfd 登记；不声明 Agent ready |
| runtime.exited | 独立 pidfd / N；正常、立即、强制退出测试通过 | 真实 TUI 退出已观察；独立于 SessionEnd，无回收权/退出码 |
| security.violation（暂名） | 实验性 Bash tool_guard；有界原生案例已验证，final 未支持 | 真实 TUI 变换/扫描/审批及拒绝/错误通过；required/final 缺口保留 |
| coverage.changed | owner 采样 / N；reset/gap/退出测试通过 | 真实缺启动、压缩、reset、退出变化已观察；OS/引擎待接入 |

未接通来源在配置准入时拒绝；仍是缺口，不能计为已完成。全部 16 项及各项效果分别验收。
真实探针使用隔离 HOME/config、禁止代理出口、无凭据的 print 模式：登录失败，只有
SessionEnd 被观察到。它只证明该结束来源，不证明 TUI 验收、模型发送或完整 AW 原生链。
合成 PTY 测试覆盖真实 cosh 格式 2 启动器、12 个 Hook 注册、Core Journal 与清理。

## 真实自然 TUI 验收

固定 1.1.47 经真实 cosh Bash 前台 PTY 启动，使用隔离 HOME/config 和已有登录态的临时副本，
保留原生 default 审批。用户级 peer Hook 仅允许三条无害命令并要求审批；AW 格式 2 通知
通过 --settings 共存。一次性批准产生预期 marker；拒绝后无 marker、无 post-tool 回调；
批准 exit-7 命令后收到 PostToolUseFailure，映射为 tool.after。
手动 /compact 产生前后回调及 SessionStart(compact)，保持代次 1；/clear 产生 end/start，
进入代次 2。原生 /exit 返回 0，owner 独立产生 runtime.exited 和退出覆盖；历史 gap 保留。
控制器在原生退出后达到 420 秒时限并回收所属进程。临时凭据、原生历史、自动更新文件和
原始终端日志已删除，原登录文件和固定制品摘要不变。这是 9 种原生公共事件 + 3 种 owner
事件的来源证据，不是全部效果验收。本 fixture 未配置 tool_guard；自然 TUI 安全链与审批共存见下方续验；排队输入、自动/失败压缩和子 Agent 重复停止检查仍待验收。

后续真实 TUI 经正常审批，用原生 Agent 工具创建一个无工具子 Agent；SubagentStart/Stop
携带同一子身份，关联父 session/runtime。原生退出后精确 --resume 恢复同一 session，
新 runtime 从 attachment 代次 1 接入，两次运行分别产生 owner 退出事件。此探针不认证
子工具调用、后台子 Agent 或重复停止检查。

## 模型发送接线缺口

[CLI 自定义模型文档](https://docs.qoder.com/cli/custom-models)描述 /model Custom 向导。
已查看固定 TUI 按账户返回的 provider/model 目录、模型选项及 API Key 页面，未输入凭据或
保存模型；凭据之前未显示 endpoint 字段，后续字段及实际路由仍未核实。不能把 IDE 配置
套给 CLI，也不能手写未公开的代理设置。
[TypeScript SDK 参考](https://docs.qoder.com/cli/sdk/references-typescript)定义的 resolveModel
上下文为 purpose、sessionId、availableModels；选择结果没有暴露组装请求正文，因此不能
直接满足 AW 完整请求发送前检查合同。下一步核实受支持的原生 provider endpoint/集成接口
及实际正文可见性；broker 还需分别验证路由与取消。这不阻塞其他事件，也不由观察推断
final/protected 权限。

## 工具前最终安全检查（实验性 native 切片）

评审约束为：`tool.before` 可有界并行处理互不依赖的只读动作；候选变换按明确顺序执行，
不能以最后返回者覆盖参数。影响本次执行的异步动作必须收齐，再固定工具名、最终参数、cwd
等上下文及策略版本，由 security 调用 sec-core 做最后一道只读检查。

`security.violation` 暂保留名称，但目标语义修正为主动检查点；接口冻结前评审是否改为
`security.check` 或 `security.before_action`。required 检查拒绝、失败、超时或取消不得放行；
修改候选或新执行尝试重新检查。OS 违规与检查结果分别保留为证据，不替代这次主动调用。
格式 2 的 notifications 仍不接受此安全路由；独立 tool_guard 配置已接有序变换与末尾 scan-code，
不经过 notify-only。原生 allow 不输出，pass 只有候选发生变化才返回 updatedInput，
未变化则返回空响应，保留原有审批。
检查错误、重复工具发生和取消返回 deny；required/final 仍未支持。

变换与可选通知仍限单命令一秒。scanner 可显式配置最多 2000ms，供版本核对与 scan-code
共用，实际只得到原工具前两秒期限的剩余时间；更短的已有配置保持有效。真实 Python CLI
启动曾使正常 pass 超过旧的一秒检查器上限；本次仅允许配置较大份额，不重试、不重开整链预算。

AW 的末尾检查仅保证自身链内顺序；Qoder 的其他 Hook 或审批若仍能修改参数，必须在实际
消费边界复检或证明消费同一候选，才可声明 final。原生拒绝及故障行为未验证时拒绝 required
准入。已加入有序变换、变换后违规、检查器失败、超时回收、取消/迟到返回、并行调用隔离与
只读查询测试。只读动作并发尚未实现；真实禁止动作未发生才算
Qoder 阻断验收。通知与安全链共用工具前两秒单调期限，Core 不重开预算。


## 原生消费与故障边界

有界 Qoder 1.1.47 print 探针使用隔离 HOME/config、已有登录态的临时副本与合成 Bash 命令。
此 fixture 以 bypass_permissions 排除审批提示干扰，入口仅接纳一条精确的合成输入。
结论限于原生 Hook 消费，不代表自然 TUI 启动器或正常审批行为验收。

| 原生案例 | 观察结果 |
| --- | --- |
| updatedInput | 只有替换后的 marker，PostToolUse 也携带替换后参数 |
| 显式 deny | 无命令 marker、无 PostToolUse |
| 两个修改参数的 Hook | 两者都看到原始输入；后返回 Hook 的 marker 出现 |
| Hook 退出 1 或 SIGKILL | 原始命令实际执行 |
| Hook 退出 2 | 无命令 marker、无 PostToolUse |

配置 tool_guard 后，AW 启动器为 PreToolUse 选择专用 --aw-guard 入口。输入解析错误、
binding 缺失、guard 配置缺失或回调事件不匹配均退出 2；可选 --aw-hook 错误仍只报告诊断，
不阻断原生行为。guard 入口无法把自身 SIGKILL 转换为退出 2。

这些实测反例排除了当前 final/required 保证：需要在实际消费边界绑定已检查候选，并在检查器
无法返回时阻断。仅把 AW Hook 排最后不能建立这两项保证。继续拒绝 required_safety，
query 保持 configured_not_certified。运行与覆盖生产者可以独立推进，model.before_request 仍为独立缺口。


完整检查链另以真实 agent-sec-cli 0.12.0 验证：安全命令变换后通过并被 Qoder 执行；
包含策略匹配文本的无害 echo 命令返回 warn，AW 拒绝，所有执行 marker 均未出现。
binding 缺失时真实 helper 退出 2，Qoder 同样未执行。Core Journal 保留
check_passed/policy_denied 与摘要；native_execution 仍为 unconfirmed，因为这些探针证据尚未成为
产品内的消费证明。探针人工构造可信 binding 后 exec 原生 CLI，没有替代自然 TUI 准入测试。


## 自然 TUI 安全链验收

真实 sec-core 0.12.0、scanner timeout_ms=2000 下通过四个有界场景：安全候选变换后仍需
原生审批，批准只执行变换结果；扫描通过后原生审批拒绝，无动作；变换为含策略匹配文本的
无害 echo 后 policy_denied；故意使变换失败后 check_failed_or_cancelled。后两者均未进入
原生审批或工具完成。用户级原生 Hook 与 AW 通知始终开启。此前 1000ms 配置的安全命令
因版本核对与扫描超出限额而拒绝，未将该次误记为策略拒绝或成功验收。
原命令、被拒绝命令和失败命令的执行 marker 均不存在。这些 fixture 证据不把产品内的
native_execution=unconfirmed 升级为消费证明，也不关闭后续 Hook/SIGKILL 的 final 缺口。
临时凭据、会话、扫描器环境、终端日志和所属进程均已清理。

## 独立 owner 生产者

格式 2 的三项 owner 路由复用 Core notify-only 和有界 Host，以 runtime_owner 来源和独立
owner-journal 领取事件。cosh 在配置阶段启动 worker；shim 完成准入后发布登记请求，
worker 验证原 owner 配置、直接 shell 子进程的 PID/start ticks，打开 pidfd 后原子确认。
shim 最多等五秒再 exec；已登记的 exec 失败保留到 owner 观察根退出。无第二个 wait/reap owner。

runtime.observed 是 exec 前登记，不是启动成功；runtime.exited 只认 pidfd 终态，
退出码、任务成功和后代回收均不推断。coverage.changed 比较 owner 的采样状态，
包括原生接入、会话代次、gap、通知失败及退出；不把 SessionEnd 当进程退出，不将侧栏状态
纳入保护覆盖。监视器/运行状态写入独立只读快照，查询不投递事件。

worker 观察窗口 24 小时；有界通知之间以 100ms 间隔采样，每 shell 最多 128 次登记、
每 runtime 最多 1024 次覆盖变化。失败不重试，覆盖通知自身失败不会递归调度。
Drop 取消并 join，关闭 pidfd；不重启旧 worker 补投，不承诺 cosh 崩溃后的交付。
合成 PTY 覆盖无 SessionEnd 的 SIGKILL、立即退出、reset/gap、双 pane 和取消回收；
真实 Qoder 自然 TUI 已验证登记、缺启动、压缩、reset 和退出快照；AgentSight 与 OS coverage 联合验收仍待完成。

## 输入提交响应边界

格式 2 可选配置一个 input_response 命令，与通知路由分开授权。Contracts 定义严格的
input.submit.respond v1 响应；Core 领取并约束发生；Host 核对 pin、执行和回收进程；
Qoder adapter 只映射上下文或拒绝。launcher 选择独立 --aw-input helper，将畸形输入或
缺 binding 映射为原生 exit 2。原提示词不可改写，许可不变，Turn 仍未知。
通知与响应共享回调期限，并复用同一个 Core 实例。

命令输入将已认证事件包裹为 `{"format":1,"scope":"input.submit.respond","event":{...}}`。
输出只允许 continue 加可选 additional_context，或 reject 加必需 reason。
命令限一秒，且受回调两秒共享期限约束。Journal claim 在失败及配置变化后保留，只记录
元数据和决策类别，不保留原始上下文或原因。reset/会话结束会阻止旧响应交付。
即使探针独立证实采用，Journal 仍显式保留 native_adoption=unconfirmed。
详见[配置合同](../../../../docs/user-guide/zh/user-entrypoint/aw.md#实验性输入提交响应)。

固定 1.1.47 print 探针已验证上下文采用、策略拒绝、畸形输出、超时、协作取消、缺 binding、
两份上下文 Hook 和其他 Hook 拒绝。杀死 helper 后原生输入处理仍继续；此反例排除
final/protected 认证。排队/补充输入语义及任意插件组合仍有缺口。自然 cosh TUI 已从 Stop
观察上下文标记，策略/畸形/超时输入未再产生 Stop，原生/runtime/shell 正常结束。
该探针证据不把元数据 Journal 变成采用回执。

## 请求端点核实

固定制品包含 modelConfigs.customModels 的本地配置说明，必需 provider/apiKey/model，
可选 baseURL/key/format。相比 Custom 向导，这是更具体的路由线索，但仍不证明直接发送接点。
隔离探针配置 OpenAI 格式模型、合成 key 和 loopback baseURL：无登录时准入失败；使用已有
登录的隔离副本后报 Failed to generate custom pool，本地服务收到零请求。
探针所填 provider/model 组合尚未对照账户目录核实，不能由此判定所有 endpoint 配置均不支持。
下一步需取得受支持目录组合或有文档的 provider 集成，再验证完整正文可见、改写和拒绝无发送。
本增量未添加生产 broker 或 model.before_request 来源。


## 主 Agent 停止响应边界

stop_response 是独立的 turn.stop.respond/v1 权限，不能由通知 ACK 获取，也不影响
SubagentStop。Core claim、Host pin/进程回收、单次初始化、共享期限和 attachment fencing
沿用输入响应的分层模式，但停止失败不能映射为退出码 2：Qoder 会据此继续调用模型。
有效响应仅允许 allow_stop 或 continue+reason；失败/取消/重复检查映射为 continue=false
及诊断字段。stop_hook_active=true 时不运行响应命令，Journal 记录 skipped，而非通过。
普通检查的决策和失败均仅记录元数据，native_adoption 保持 unconfirmed。

这是一条可选原生响应链，不是安全检查终态证明。限制依赖 Qoder 的重复检查字段；其他 Hook
可能改变原生最终行为，helper 被杀也不能保证生效。固定 Qoder 1.1.47 的 11 个 print 案例
已验证 allow_stop、继续理由采用、重复检查停止、畸形/超时/取消/缺 binding，以及另一 Hook
请求继续对比 AW 允许/失败的两种声明顺序；两种顺序中 AW 失败响应均停止 peer 继续请求。
杀死 helper 后只留下 started Journal，原生输出仍结束。自然 cosh TUI 在用户级 Hook 共存下
连续两次输入均采用继续理由，新输入重置 stop_hook_active，随后畸形/超时检查没有额外回答。
TUI 显示了诊断，print 则隐藏诊断且失败仍退出 0，已经输出的回答不会撤回。这些原生观察
不把元数据 Journal 变为消费回执，也不认证任意插件组合。TUI /exit 后独立观察到
runtime.exited，再由 shell 完成清理。配置详见[用户指南](../../../../docs/user-guide/zh/user-entrypoint/aw.md#实验性主-agent-停止响应)。

端点补充：当前 [CLI 文档](https://docs.qoder.com/cli/custom-models) 要求通过账户 Custom
向导配置，不应手写 settings.json。官方 [公告](https://forum.qoder.com/t/qoder-cli-now-supports-custom-models/12444)
将通用 base URL/model ID/API key 接入标为从 1.1.50 开始；这不能倒推 1.1.47 的能力。
下一步应在隔离制品上验证新版本兼容与合法入口，或取得固定版受支持的 provider 接口；本次未升级主机或建立代理。

## 交互工具结果响应边界

`tool_response` 在通知之后向单个固定命令授予独立的 `tool.after.respond/v1` 权限，
仅允许 preserve，或匹配原结果摘要后替换已有的 Bash completed stdout 槽位。调用方
显式接受 unrecoverable 输出；Journal 只留 metadata/摘要，不留正文。复用已有原生
提取器和替换格式化逻辑。

不能直接复用 `ProjectionSettings` 作为交互绑定：它要求 owner 认证的单轮 Turn 以及
精确历史/留存配置。本切片保留未知 Turn，不自动继承 `qoder-project` 的 sec-core、
Tokenless、候选留存或历史采用证据。

Core 在命令执行前认领，共享原回调期限，校验输出并确认完成记录后才返回。owner 按
session epoch/tool call 一次认领成功或失败，即使没有通知路由也不允许再次认领；不支持的
终态结果不能随后改成成功回调来重试替换。迟到、取消或过期候选不交付。可选投影失败
保留原始结果，不是强制脱敏屏障。

固定 Qoder 1.1.47 已完成八个真实 print 案例：preserve、replace、两种 peer 声明顺序、
畸形输出、超时、取消和 helper SIGKILL。peer 声明在 AW 后面时，后续回答采用 peer 标记；
反向排列则采用 AW 标记。两种顺序中 AW 收到的均为原始 stdout，不能将其理解为各 Hook
依次处理前一个候选的组合合同。可返回的失败保留原文并返回诊断；SIGKILL 只有 started，
Qoder 仍根据原文回答并退出 0。

print 探针使用测试构造的进程绑定。另一个自然 cosh→Qoder TUI 会话使用当前 Rust
owner/launcher，两个输入依次验证 preserve 和 replace；Stop 回调确认对应的后续回答
标记，包括未出现在 prompt/原文中的替换标记，并观察到 runtime.exited。
未断言渲染后的回答区域，也未核验原生历史。Turn 保持未知，不因本次案例提升 query/Journal
采用状态。错误 source digest 仍仅有合成覆盖；此前阶段未覆盖交互 Tokenless/sec-core
provider 或按需 Herdr。后续章节说明新增实现及其独立验收范围；任意插件组合和
final/protected 保证仍待验收。

## 按需终端归属

安装制品、注册/选择系统登录 shell、当前会话启用 AW/Herdr 是独立边界，参见
[#3373](https://github.com/agentic-os-org/ANOLISA/issues/3373)。按需入口不要求修改
`/etc/shells` 或账户登录 shell；登录进入 cosh 本身也不表示启用 AW 或打开 Herdr。
外层登录 shell 的启动语义由 ShellHost 负责，Herdr 内层 pane 使用非登录 shell，
不能通过继承外层标志再次执行登录 profile。接入其他 ShellHost 启动路径时，必须保留
marker 与 AW shim 的同一启动链，并分别验收手动/登录进入、AW 开/关以及退出恢复。

cosh runtime 的可选 Herdr 启动器保留外层 Bash，仅在前台输入 `qoder` 时打开本次私有、
固定 v0.9.0 server/client；打开 UI 前调用与原生启动共用的准入检查。
Herdr pane 直接运行当前 cosh 二进制，该进程建立自己的 AW scope 和原生 Bash；
Bash 等待 helper exec 成 Qoder。pane 只承载原生终端，不选择另一个模型 adapter。

0700 私有会话目录保存 argv 字节、launcher/server 身份和 pane 绑定。shell 就绪后，
固定内部命令一次性认领启动，用户参数不进入 shell 文本。只有绑定的 pane owner 能发布
完成结果，Herdr 重启 shell 不能覆盖 Agent 退出码。内层 AW/shell 临时文件归入该会话目录，
便于异常收尾。pane 恢复原 XDG 环境，不改写用户 Hook 或原生配置；固定 cwd 准入保持不变。

启动器为启动、RPC、会话时长和 owned child 清理设置边界。先请求 Herdr 关闭 workspace，
再检查已登记后代身份，对剩余对象通过 pidfd 发信号。正常 Qoder 仍由原生 Bash 回收；
这不是任意脱离进程的隔离，也不构成 final/protected。SIGHUP 取消启动器；内层 Bash 的
一次性 INT trap 在 Agent 被 SIGINT 终止时保证返回外层 shell。shell 就绪失败时回收 Bash，
不向启动脚本读者注入 Agent 命令。当前只支持单 pane 自动启动，多 pane 未认证。

显式 ignored 的 shell_host 测试使用官方固定 Herdr 和合成 Qoder，核对 metadata、归属、
参数、退出码和清理；登录回归额外验证 pane 不重放外层 profile。真实 Qoder 1.1.47
已从此入口运行，后续回答的投影标记采用由 Stop 回调核实；不将该证据等同于
终端渲染、排他脱敏或单次完整产品演示。组合产品视觉验收仍独立记录。
Rust 运行入口不依赖外部 Python/shell 启动脚本；安装期获取和测试驱动仍单独管理。

## 原生组件 profile 与效果证据

`aw-hook-cli configure` 接纳仓库内默认策略，核对安装版本/pin 后生成私有格式 2 profile。
用户 `[aw]` 引用通过 ShellHost 的环境覆盖到达 Bash/pane，不修改进程全局环境，
也不在日常启动时更新信任摘要。Qoder 在 cosh 内层 PTY 中运行，Herdr 外层 pane 的进程检测
不一定能把它识别为 Agent。bridge 核实 pane 归属，且 workspace 只有该 pane 时，才把相同
只读 tokens 发布到 workspace；工作区行展示真实事实，不注入 Agent 身份/状态。pane metadata
继续供已有消费者使用。生成的侧栏配置显式设置最大宽度和隐藏折叠模式；metadata 本身不证明
真实画面可见。

原生 Tokenless 后端独立于单 Turn 的 `qoder-project` 启动器。它只把经过认证的主 Agent
成功 Bash 结果映射给已有投影 codec，保留未知 Turn 身份，复用回调预算、Core claim、
journal 和 attachment 隔离，不补造 Turn 或历史采用回执。

只读效果计数核对 journal 摘要链，按 runtime/session/epoch 过滤，区分检查、替换候选和
原样保留。默认没有 observer 命令，因此存在回调但通知命令计数为零是合法状态。
产品 metadata 的原生采用仍未确认；外部验收可另外绑定精确原生历史结果摘要。
见[验收流程](../../../../docs/developer-guide/zh/aw/qoder-acceptance.md)。
