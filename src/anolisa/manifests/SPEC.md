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

DistributionIndex 使用扁平 `[[entries]]` 数组表达具体 artifact，一行
对应一个 (component, version, channel, target) 绑定。Loader 实现见
`anolisa_core::distribution::DistributionIndex`。

### 顶层 meta 字段

| 字段 | 必填 | 说明 |
|---|---|---|
| `schema_version` | 是 | DistributionIndex schema 版本，当前固定为 `1` |
| `channel` | 否 | 默认 channel；entry 上显式 `channel` 覆盖该默认值 |
| `generated_at` | 否 | ISO-8601 时间戳，便于排查版本漂移 |
| `expires_at` | 否 | 过期时间，企业内网 index 可不设置 |
| `publisher` | 否 | 发布主体，例如 `"anolisa"` |
| `signature` | 否 | index 级签名方式，例如 `"cosign"` |

### `[[entries]]` 字段

| 字段 | 必填 | 说明 |
|---|---|---|
| `component` | 是 | 对应 component manifest 的 name |
| `version` | 是 | 与 component 对齐的版本号 |
| `channel` | 是 | `stable / beta / nightly / dev` |
| `artifact_id` | 否 | 稳定 artifact 标识，便于审计 |
| `manifest_digest` | 否 | 对应 component manifest 的摘要，防止漂移 |
| `artifact_type` | 是 | 枚举：`rpm` / `deb` / `tar_gz` / `zip` / `oci` / `file` / `binary`（snake_case，禁止 `tar.gz`） |
| `backend` | 是 | 下载/安装 backend，例如 `github-release` / `yum-repo` / `aliyun-oss` |
| `url` | 是 | artifact 下载地址或 repo locator |
| `os` / `os_version` | os 必填 | OS 选择器；`os_version` 可为常量或 `>=4` 这类约束 |
| `arch` | 是 | `x86_64` / `aarch64` / `any` |
| `libc` / `pkg_base` | 条件必填 | Linux artifact 通常需 `libc`；系统包 artifact 需 `pkg_base` |
| `install_modes` | 是 | `user` / `system` 列表 |
| `sha256` / `signature` / `signature_url` | 推荐 | artifact 校验 / 签名 |
| `size` | 否 | 字节大小（描述性） |
| `dependencies` | 否 | backend 级依赖，例如 RPM 依赖 |

### 兼容口径

`artifact_type` 的 loader 仍接受历史 `tar.gz` / `tar` 文本，但会归一化为
`tar_gz`；新写入的 manifest 必须使用 `tar_gz`。

## AgentSight P0 要求

`agentsight` 和 `agent-observability` 相关 manifest 必须优先补齐：

- system mode host 安装路径。
- rootless/container 降级状态。
- eBPF/BTF/CAP_BPF gate。
- health check。
- central log / component reported log source。
