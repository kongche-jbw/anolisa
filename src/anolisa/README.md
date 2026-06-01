# ANOLISA CLI Development Map

本目录是 `anolisa` CLI、核心库、manifest 和模板的开发入口。完整背景见
`docs/anolisa/anolisa-cli-launch-spec.md` 和
`docs/anolisa/anolisa-framework-contract.md`。

当前实现状态、可用命令和下一步优先级见
`docs/anolisa/anolisa-cli-development-status.md`。

## 目录分工

| 路径 | 内容 | 对应 launch spec |
|---|---|---|
| `crates/anolisa-cli/SPEC.md` | 命令面、默认人类可读输出、`--json`、`NOT_IMPLEMENTED` | S1, S2 |
| `crates/anolisa-core/SPEC.md` | Catalog、resolver、planner、state、central log、rollback | S3, S5, S6, S8 |
| `crates/anolisa-env/SPEC.md` | `anolisa env`、EnvFacts、环境 gate、target probe | S4 |
| `crates/anolisa-platform/SPEC.md` | 文件布局、权限、服务、包管理、DistributionIndex 下载落地 | S7, S8 |
| `crates/anolisa-build/SPEC.md` | `--from-source`、`runtime build`、legacy build-all backend | S7 |
| `manifests/SPEC.md` | Capability、Component、Adapter、DistributionIndex manifest 口径 | S3, S7, S9, S10 |
| `templates/` | 可复制的 TOML 模板，不参与当前 catalog 加载 | S1, S3, S7, S8, S9 |

## 当前结论

- `anolisa` 是本地管家入口，默认输出优先人类可读。
- `--json` 是可选机器输出，必须复用同一 response model。
- 已公开但未实现的命令返回 `NOT_IMPLEMENTED`。
- 组件默认走预编译产物；源码构建只用于开发或显式 `--from-source`。
- component manifest 只声明安装选择策略；具体 artifact URL、checksum、signature、backend 放入 DistributionIndex。
- AgentSight / `agent-observability` 是 P0。

## 开发顺序

1. 先看 `crates/anolisa-cli/SPEC.md`，固定命令语义和输出契约。
2. 再看 `manifests/SPEC.md` 与 `templates/`，固定 manifest 和 index 字段。
3. 然后看 `crates/anolisa-core/SPEC.md`，实现 catalog、planner、state、central log。
4. 最后按功能接 `env/platform/build` 的具体能力。

## 注意事项

- 不要把模板 TOML 放进 `manifests/runtime`、`manifests/osbase` 或 `manifests/capabilities`，避免被当前 loader 当成真实 manifest。
- 新增真实组件 manifest 前，先复制 `templates/component-runtime.toml` 或 `templates/component-osbase.toml`，再放到对应 manifest 目录。
- 新增分发 artifact 前，先补 DistributionIndex，不要把下载 URL 写死在 Rust 命令逻辑中。
