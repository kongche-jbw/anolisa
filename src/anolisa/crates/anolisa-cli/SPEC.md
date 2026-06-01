# anolisa-cli Spec

对应 launch spec：S1 Command Surface Spec、S2 UX and Output Spec。

## 当前结论

- `anolisa-cli` 只负责参数解析、上下文组装、输出格式化和错误展示。
- 业务语义进入 `anolisa-core`、`anolisa-env`、`anolisa-platform`、`anolisa-build`。
- 默认输出面向人类；`--json` 使用同一 response model。
- 公开但暂未实现的命令返回 `NOT_IMPLEMENTED`，不能 panic 或静默成功。

## 命令分层

| 层级 | 命令 | 开发归属 |
|---|---|---|
| Tier 1 capability | `list`, `enable`, `disable`, `status`, `doctor`, `logs`, `restart`, `env`, `info`, `update` | `commands/tier1` + core service |
| Subscription | `subscription register/status/refresh/unregister` | `commands/subscription` + core subscription |
| Adapter | `adapter scan/list/install/remove` | `commands/adapter` + core TargetProvider |
| Self | `self update/adopt/completions` | `commands/self` + core self/adopt |
| Runtime | `runtime install/remove/update/build/list/status` | `commands/runtime` + core runtime planner |
| Osbase | `osbase kernel/sandbox/security` | `commands/osbase` + core osbase planner |

## 全局参数

| 参数 | 语义 |
|---|---|
| `--install-mode <user|system>` | 安装作用域，传入 `CliContext` |
| `--prefix <PATH>` | system mode prefix override |
| `--json` | stdout 只输出 JSON |
| `--dry-run` | 只生成 plan，不执行 |
| `-v/--verbose` | human 输出增加细节 |
| `-q/--quiet` | 抑制非错误 human 输出 |
| `--no-color` | 禁用颜色 |

## 输出契约

每个 handler 返回统一 response：

```text
Response {
  command,
  status,
  summary,
  details,
  warnings,
  advice,
  plan,
  data,
}
```

规则：

- human formatter 只展示最重要摘要、状态、下一步建议。
- JSON formatter 输出完整结构。
- `--json` 时 stdout 只允许 JSON，日志和 debug 进入 stderr。
- 错误也有 JSON 形态，至少包含 `code/reason/advice`。

## 已确认命令语义

- `logs [OBJECT]` 是中心化日志过滤入口，包含 ANOLISA operation/audit logs 和组件上报日志。
- `doctor` 无参数是只读检查；`doctor --fix` 直接修复；`--dry-run --fix` 只输出修复计划。
- `update self` 独立更新 CLI；`update all` 不包含 `self`。
- `enable --feature <name>` 表示先启 capability，再启指定 feature。
- `runtime install --component-version <VERSION>` 替代会冲突的 `--version`。

## 模板

- `../../templates/command-spec.toml`
