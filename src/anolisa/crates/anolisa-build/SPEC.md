# anolisa-build Spec

对应 launch spec：S7 Distribution Spec 中的 source-build fallback。

## 当前结论

- 客户默认路径不走源码构建。
- `build-all.sh` 只保留为 develop 场景、CI 兼容入口和 `--from-source` 后端。
- 组件的 build 字段服务于开发、CI、应急源码构建和产物生产，不是客户首选路径。

## 入口命令

| 命令 | 语义 |
|---|---|
| `anolisa runtime build <component|all>` | 开发构建，不必安装 |
| `anolisa runtime build --no-install` | 只产生产物 |
| `anolisa enable <capability> --from-source` | 显式 source install |
| `anolisa runtime install <component> --from-source` | 显式 source install |

## Build Backend

| backend | 说明 |
|---|---|
| cargo | Rust component |
| make | C/C++/eBPF/mixed component |
| npm | JS adapter/tooling |
| static | 已存在静态文件或脚本 |
| legacy-script | 兼容 `build-all.sh` 或历史脚本 |

## 产物契约

build backend 只负责产出 declared outputs：

```text
build.outputs -> install.files
```

安装阶段只消费 `install.files`，不反向猜测构建产物。

## 验证

- source build 失败不能污染 installed state。
- `--from-source --dry-run` 只展示 build plan。
- legacy backend 必须被 transaction 包裹，输出进入 central log。
