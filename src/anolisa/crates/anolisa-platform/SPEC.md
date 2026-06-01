# anolisa-platform Spec

对应 launch spec：S7 Distribution Spec、S8 State and Audit Spec。

## 当前结论

- `anolisa-platform` 负责 OS 差异、文件布局、权限、systemd、package manager、下载落地。
- 不负责 capability 业务决策。
- DistributionIndex 解析后的 artifact 安装动作在这里落地，但 resolver 在 core。

## FsLayout

| 类别 | system mode | user mode |
|---|---|---|
| bin | `/usr/local/bin` 或 `--prefix/bin` | `$XDG_BIN_HOME` 或 `~/.local/bin` |
| state | `/var/lib/anolisa` | `$XDG_STATE_HOME/anolisa` |
| config | `/etc/anolisa` | `$XDG_CONFIG_HOME/anolisa` |
| cache | `/var/cache/anolisa` | `$XDG_CACHE_HOME/anolisa` |
| logs | `/var/log/anolisa` | `$XDG_STATE_HOME/anolisa/logs` |
| lock | `/var/lib/anolisa/lock` | `$XDG_STATE_HOME/anolisa/lock` |

## Package Manager

需要抽象以下 backend：

| backend | 场景 |
|---|---|
| rpm file | GitHub Release 或 OSS/CDN 上的 `.rpm` |
| yum/dnf repo | 批量企业更新、依赖解析 |
| deb file | Debian/Ubuntu `.deb` |
| apt repo | Debian/Ubuntu 企业分发 |
| tar.gz/zip | user-mode、rootless、macOS、container |
| local-file | 离线安装、内网交付 |

## Distribution 安装规则

- 先校验 index signature。
- 再校验 artifact sha256 和 signature。
- 下载进入 cache，不直接覆盖目标路径。
- system service 操作必须可 dry-run。
- 安装、升级、删除都必须写 state 和 central log。
- 修改外部文件前必须调用 backup store。

## Central Log 路径

```text
user-mode:   $XDG_STATE_HOME/anolisa/logs/operation.log
user-mode:   $XDG_STATE_HOME/anolisa/logs/component.log
system-mode: /var/log/anolisa/operation.log
system-mode: /var/log/anolisa/component.log
```

## 模板

- `../../templates/distribution-index.toml`
- `../../templates/installed-state.toml`
