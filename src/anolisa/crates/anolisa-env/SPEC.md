# anolisa-env Spec

对应 launch spec：S4 Env Probe Spec。

## 当前结论

- `anolisa env` 第一优先级是人类可读。
- `--json` 输出完整 EnvFacts。
- env probe 只负责探测事实，不决定安装策略；安装策略由 core planner 消费 EnvFacts。
- target framework 探测规则放在 `probes/`，不放进 CLI 命令分支。

## EnvFacts 最小字段

```text
os.id
os.version
arch
libc
kernel.version
pkg_base
install_mode_supported
privilege.user
privilege.root
systemd.available
container.detected
container.rootless
virtualization.kind
kvm.available
btf.available
capabilities.available
frameworks.openclaw
frameworks.hermes
frameworks.codex
frameworks.claude_code
frameworks.qwen_code
```

## Probe 分类

| Probe | 责任 |
|---|---|
| os | `/etc/os-release`、arch、libc、pkg_base |
| kernel | kernel version、BTF、eBPF 相关能力 |
| privilege | root/user、capability、sudo/system install 可用性 |
| container | container/rootless/unprivileged 场景 |
| virtualization | KVM、VM、bare metal hint |
| package_manager | rpm/deb/yum/dnf/apt 可用性 |
| frameworks | OpenClaw/Hermes/Codex/Claude Code/Qwen Code 探测 |

## Gate 输出

Env gate 不直接报“不能装”，而是输出 capability/component/feature 的状态：

```text
available
degraded
blocked
unknown
```

每个非 available 状态必须带 `reason` 和 `advice`。

## 验证

- `anolisa env` human 输出适合快速阅读。
- `anolisa env --json` 可作为 planner 测试 fixture。
- container/rootless/macOS 场景必须给出明确降级原因。
