# Cosh Agent Memory

[English](../../en/token-saving/cosh-agent-memory.md)

Agent Memory 会为 Cosh 留下一份私有、持久的工具证据记录。新 session 可以找回当前工作需要的内容，无需重放完整 transcript。安装完成后，Cosh 会自动接入；Memory 暂时不可用时，当前任务仍可继续。

## 使用条件

- Linux
- 需要自动捕获和召回时安装 cosh-ng
- 一个可写且仅 owner 可访问的本地状态目录

ManT 是可选组件。本地捕获、召回和验证 demo 都不依赖它。

## 安装后大约 30 秒完成验证

先安装 Agent Memory。下载和安装 package 的时间不计入这 30 秒本地验证。

```bash
anolisa install agent-memory
```

随后在平时使用 Cosh 的 workspace 中运行下面几条命令。

```bash
agent-memory-ctl doctor
agent-memory-ctl demo
agent-memory-ctl status
```

`doctor` 会检查私有本地存储，并确认 Cosh Hook executable 已经出现在 `PATH` 中。它也会报告 Cosh 和可选 ManT command 是否存在。

`demo` 会写入一条合成的非敏感 event，关闭并重新打开持久后端，随后在第二个本地 session 中把它召回。这个过程不会执行 shell command，也不会打印存储内容。成功后，CLI 会显示 cold reopen 耗时和 ContextView ID。复制它最后打印的命令即可解释本次选择。

```bash
agent-memory-ctl why local-view-1
```

`status` 会显示持久化对象数量、硬容量、保留周期、SQLite 的 logical 和 physical 用量，以及有界的近期召回漏斗。漏斗把“返回过内容的 view”和“明确报告 useful 的 outcome”分开统计。合成 `demo` view 会单独报告，不会被算成真实 Agent 命中。命令不会打印数据库路径或 Memory 内容。

安装后重新启动一个 Cosh session。Cosh 会自动发现随包安装的 extension，这条接入路径不需要复制 MCP 配置。

## 其他安装方式

Alinux 用户也可以安装 RPM package。

```bash
sudo yum install agent-memory
```

从源码构建的开发者可以用 Make 安装同一组 binary 和 Cosh extension。

```bash
cd src/agent-memory
make build
make install
```

使用 user profile 从源码安装时，请确认 Cosh 继承的 `PATH` 中包含 `$HOME/.local/bin`。

## Cosh 会记录什么

工具调用成功或失败后，Hook 会记录一条有长度限制且经过脱敏的 event summary，其中含有 hash 和不透明的 evidence reference。这条路径不会保存完整 transcript。`AfterModel` 和 `Stop` 产生的 model output 也不会被当作已经提交的 Memory。

在 `SessionStart` 阶段，Agent Memory 可以返回同一 local user、Agent 和 workspace scope 中近期的 Candidate evidence。在 `UserPromptSubmit` 阶段，它会挑选与当前 prompt 有词项重合的 Candidate evidence。返回内容受 item、byte 和 Token budget 限制，还会再次检查 secret 和 prompt injection。Cosh 注入 model context 前，会用固定标记把它包成不可信数据。

Cosh 会把固定 project boundary 和可变 shell cwd 分开传递。Agent Memory 会把 Git worktree 内的路径归一化到 canonical worktree root，因此进入仓库子目录不会切断召回，也不会让 `why` 和 `forget` 找不到同一个 view。相邻 worktree 仍保持隔离。

Memory 故障采用 fail-open 行为。backend 不可用或超过 deadline 时，Cosh 会跳过召回内容并继续当前任务。

## Command 说明

| Command | 结果 | Exit 行为 |
|---|---|---|
| `agent-memory-ctl doctor` | 检查本地存储、必需的 Cosh Hook、Cosh runtime 和可选 ManT protocol | 必需检查失败时返回非零值 |
| `agent-memory-ctl demo` | 在 cold backend reopen 后捕获并召回合成 evidence | 捕获、重开、召回或 outcome 记录失败时返回非零值 |
| `agent-memory-ctl status` | 显示 byte、容量、生命周期、对象计数、近期召回指标和被排除的诊断样本 | 无法检查本地存储时返回非零值 |
| `agent-memory-ctl why <view-id>` | 显示 item ID、排名、准入原因、降级和 outcome，不显示存储内容 | view 不存在或不属于当前 workspace 时返回非零值 |
| `agent-memory-ctl forget <kind> <id> --yes` | 删除当前 workspace 中的 task、event 或 ContextView | 未确认或没有删除可见对象时返回非零值 |

每条 command 都可以在 subcommand 后使用 `--json`。

```bash
agent-memory-ctl doctor --json
agent-memory-ctl demo --json
agent-memory-ctl status --json
agent-memory-ctl why local-view-1 --json
agent-memory-ctl forget context-view local-view-1 --yes --json
```

JSON 输出只包含状态、计数和安全诊断。错误会写入 stderr，同时给出处理动作并返回非零 exit code。

`forget` 是破坏性操作，因此必须显式传入 `--yes`。删除 ContextView 时，其准入和 outcome 记录也会一并删除。若同一 workspace 的多个 session 出现相同 event ID，命令会拒绝猜测并返回冲突。

## 常见处理办法

如果 `doctor` 报告 Hook 缺失，请重新安装 Agent Memory，并检查 Cosh 继承的 `PATH`。executable 名称为 `agent-memory-cosh-hook`。

如果本地存储检查提示目录必须仅 owner 可访问，请把它的上级状态目录权限设为 `0700`，随后重试。CLI 会主动省略数据库路径；确实需要定位目录时，请检查自己的 `XDG_STATE_HOME` 配置。

如果没有找到 Cosh，本地 demo 和 status 仍然可以运行。安装 cosh-ng 并新建 session 后，Cosh 才会自动捕获和召回生命周期事件。

如果没有找到 ManT，无需处理。ManT 是可选 Knowledge Provider，不是本地 Memory 的 runtime dependency。
