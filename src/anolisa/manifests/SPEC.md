# Manifest Spec

对应 launch spec：S3 Manifest Schema Spec、S7 Distribution Spec、
S9 Target Provider Spec、S10 AgentSight P0 Spec。

## 当前结论

- `manifests/capabilities` 存放用户视角 capability。
- `manifests/runtime` 存放 runtime component。
- `manifests/osbase` 存放 OS/base substrate component。
- 具体 artifact URL、checksum、signature、backend 不写入 component manifest，统一进入 DistributionIndex。
- TOML 模板放在 `../templates`，不放进真实 manifest 目录。

## 目录

| 路径 | 说明 |
|---|---|
| `capabilities/*.toml` | capability -> components/features/backends |
| `runtime/*.toml` | runtime component，例如 agentsight/tokenless/ws-ckpt/sec-core |
| `osbase/*.toml` | kernel/sandbox/security substrate |
| `../templates/capability.toml` | capability 模板 |
| `../templates/component-runtime.toml` | runtime component 模板 |
| `../templates/component-osbase.toml` | osbase component 模板 |
| `../templates/target-provider.toml` | target provider 模板 |
| `../templates/distribution-index.toml` | DistributionIndex 模板 |

## Capability Manifest

必须表达：

- capability name/description。
- 需要的 components。
- capability 默认 features。
- capability-level env requirement。

## Component Manifest

必须表达：

- component name/version/layer/domain。
- source path。
- distribution selectors。
- build backend 和 outputs。
- install modes/files/services/capabilities。
- environment requirements。
- dependencies。
- features。
- adapters。
- health checks。

## DistributionIndex

DistributionIndex 表达具体 artifact：

- component name/version。
- channel。
- artifact type。
- backend。
- url。
- os/arch/libc/pkg_base。
- install modes。
- sha256/signature。
- dependencies。

## AgentSight P0 要求

`agentsight` 和 `agent-observability` 相关 manifest 必须优先补齐：

- system mode host 安装路径。
- rootless/container 降级状态。
- eBPF/BTF/CAP_BPF gate。
- health check。
- central log / component reported log source。
