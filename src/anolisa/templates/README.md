# ANOLISA TOML Templates

这里放可复制的 TOML 模板，不参与当前 manifest loader。

## 文件

| 文件 | 用途 |
|---|---|
| `command-spec.toml` | 每个公开命令的 CommandSpec 模板 |
| `capability.toml` | capability manifest v2 模板 |
| `component-runtime.toml` | runtime component manifest v2 模板 |
| `component-osbase.toml` | osbase component manifest v2 模板 |
| `target-provider.toml` | TargetProvider / adapter manifest 模板 |
| `distribution-index.toml` | DistributionIndex v1 模板 |
| `installed-state.toml` | installed state 文件形态模板 |

## 使用规则

- 新增真实 capability 时，复制 `capability.toml` 到 `manifests/capabilities/<name>.toml`。
- 新增真实 runtime component 时，复制 `component-runtime.toml` 到 `manifests/runtime/<name>.toml`。
- 新增真实 osbase component 时，复制 `component-osbase.toml` 到 `manifests/osbase/<name>.toml`。
- 新增 artifact 时，优先更新 DistributionIndex，不要把 URL 写进 Rust 命令逻辑。
- 模板里的占位值必须替换，不能直接作为真实 manifest 提交。

## 命名约定

- `artifact_type` 与 `preferred_artifact_types` 必须使用 snake_case：
  `rpm` / `deb` / `tar_gz` / `zip` / `oci` / `file` / `binary`。
  历史拼写 `tar.gz` / `tar` 仍被 loader 接受，但写入新 manifest 时统一为 `tar_gz`。
- `distribution-index.toml` 使用扁平 `[[entries]]` 数组，一条记录一个
  (component, version, channel, target) 绑定。顶层 `channel` / `generated_at` /
  `expires_at` / `publisher` / `signature` 为可选 meta。
