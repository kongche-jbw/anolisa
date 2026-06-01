# ANOLISA GitHub Release Distribution

本文记录 P1-G1 阶段用 GitHub Release 分发预编译组件的最小口径。目标是让 `distribution-index` 指向 GitHub Release asset，`anolisa enable agent-observability` 能从远端下载、校验 sha256，并安装到 ANOLISA 管理路径。

## 当前结论

- GitHub Release 是 artifact 仓库；`distribution-index.toml` 是 artifact 路由表；`anolisa` 本地 CLI 负责解析 index、下载、校验和安装。
- P1-G1 只要求 `agentsight` 的 Linux x86_64 demo artifact 跑通，artifact 可以是 fake binary；真实 AgentSight 二进制后续替换同名或新版本 asset。
- `DownloadCache` 已支持 `file://`、`http://`、`https://`，仍强制由 executor 校验 `sha256`。
- 当前不做签名校验；`signature_url` / keyring / sigstore 仍是后续 P1/P2 项。
- 当前没有 `anolisa index add` 命令；测试 GitHub Release index 时，需要把 release 里的 `distribution-index.toml` 放到 overlay 路径：
  - system mode: `<prefix>/etc/anolisa/manifests/distribution-index/index.toml`
  - user mode: `~/.config/anolisa/manifests/distribution-index/index.toml`

## Release Asset 口径

建议 release tag 使用组件名和版本，例如：

```text
agentsight-v0.2.0-demo
```

最小 assets：

```text
agentsight-0.2.0-linux-x86_64
distribution-index.toml
SHA256SUMS
```

`distribution-index.toml` 使用 flat `[[entries]]` schema：

```toml
schema_version = 1
channel = "stable"
generated_at = "2026-06-02T00:00:00Z"
publisher = "anolisa-demo"

[[entries]]
component = "agentsight"
version = "0.2.0"
channel = "stable"
artifact_type = "binary"
backend = "github-release"
url = "https://github.com/kongche-jbw/anolisa/releases/download/agentsight-v0.2.0-demo/agentsight-0.2.0-linux-x86_64"
os = "linux"
arch = "x86_64"
libc = "glibc"
install_modes = ["system"]
sha256 = "<sha256-of-asset>"
dependencies = []
```

## 验证路径

在 Linux x86_64 机器上：

```bash
DEMO_ROOT="$(mktemp -d /tmp/anolisa-release-demo-XXXXXX)"
mkdir -p "$DEMO_ROOT/etc/anolisa/manifests/distribution-index"
curl -fsSL \
  "https://github.com/kongche-jbw/anolisa/releases/download/agentsight-v0.2.0-demo/distribution-index.toml" \
  -o "$DEMO_ROOT/etc/anolisa/manifests/distribution-index/index.toml"

anolisa --install-mode system --prefix "$DEMO_ROOT" enable agent-observability --dry-run --json
anolisa --install-mode system --prefix "$DEMO_ROOT" enable agent-observability --json
anolisa --install-mode system --prefix "$DEMO_ROOT" status agent-observability --json
anolisa --install-mode system --prefix "$DEMO_ROOT" logs --json
```

## 后续动作

- 增加 `anolisa index add/list/remove`，让用户不用手动放 overlay 文件。
- 把 demo fake binary 替换为真实 AgentSight release artifact。
- 增加 signature/keyring 校验，避免只依赖 sha256。
- 把 GitHub Release distribution smoke 纳入 Linux CI。

## 待决策问题

- 正式 tag 是否沿用现有 `sight/v*` release workflow，还是为 ANOLISA CLI runtime 分发新增 `agentsight-v*` 规则。
- `backend = "github-release"` 是否应该归一到 install runner 可执行 backend，还是仅作为 distribution metadata。
- `distribution-index.toml` 是作为每个组件 release 的 asset 发布，还是维护一个全局 index release。
