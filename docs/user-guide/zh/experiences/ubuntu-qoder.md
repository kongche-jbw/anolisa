# Ubuntu 上的两场 Qoder 快速体验

[English](../../en/experiences/ubuntu-qoder.md)

先展示同一份 RPM 风格 Skill 在 Ubuntu 上读出适用的命令，再展示 Qoder 用更短的
工具输出定位 CI 报告中的失败。工作人员完成准备后，每场体验控制在 2～3 分钟。
下载安装和账号登录放在会前，不计入参与者的体验时间。

| 体验主题 | 小吊牌 | 体验介绍 | 配图/素材 | 口号 | 时长 |
| --- | --- | --- | --- | --- | --- |
| SkillFS 跨发行版 Skill 体验 | 读时适配、原文不变 | 在两个 Ubuntu 工作区读取同一份 RPM 风格 Skill，对比命令并核对源文件校验值，体验无需改原文的发行版适配。 | 双终端对照图、源文件校验值、群二维码 | 系统换了，Skill 不用改。 | 2～3分钟 |
| Tokenless 工具输出减负体验 | 日志压缩、失败定位 | 让 Qoder 分别在关闭和开启压缩时诊断同一份 CI 报告，对比失败定位结果，查看工具内容的实际减负记录。 | 完整/压缩报告、统计截图、群二维码 | 少读测试日志，照样找出失败。 | 2～3分钟 |

## 准备纯净机器

使用 Ubuntu 22.04 或 24.04 **x86_64**，准备一个可使用 sudo 的普通用户、网络和
可访问的 `/dev/fuse`。当前发布的 SkillFS raw 包支持 x86_64、系统级安装；ARM
机器需要另行源码构建。快速安装脚本会在 ARM 上明确停止。Ubuntu 不需要安装 RPM
包管理器。

拉取策划者的 fork 分支并运行准备脚本。

```bash
git clone --depth 1 --branch feature/skillfs/ubuntu-experience --single-branch \
  https://github.com/kongche-jbw/anolisa.git anolisa-experience
cd anolisa-experience
bash scripts/ubuntu-experience/bootstrap.sh
export PATH="$HOME/.local/bin:/usr/local/bin:$PATH"
```

没有 Git 时先执行 `sudo apt-get update && sudo apt-get install -y git`。
脚本通过官方安装入口安装 anolisa，再执行以下组件安装命令。

```bash
sudo "$HOME/.local/bin/anolisa" --install-mode system install skillfs \
  --backend raw --version 0.4.2
anolisa --install-mode user install tokenless --backend raw --version 0.8.2
```

脚本安装 anolisa 0.3.12、FUSE3、Python/pytest；缺少 Qoder 时安装 Qoder，然后创建
体验目录并完成本地检查。网络和安装操作均有超时，失败即停止，不会自动转为源码
编译或安装其他组件。重复执行可能重新核对固定版本组件，已有 Qoder 会被保留。

Qoder 必须支持 `--config-dir`、`--plugin-dir` 和 `--no-session-persistence`，脚本
会检查。彩排时记录实际 Qoder 版本，活动期间保持该版本。

## 会前配置一次 Token Plan

```bash
python3 scripts/ubuntu-experience/demo.py auth
```

按提示完成 Qoder 登录。在 `/model` 中打开 **Custom**，选择 **Add custom model**，
根据界面实时目录选择阿里云供应商、已购买的 Token Plan 和对应模型，在向导内输入
Key，选中配置好的模型后退出 Qoder。配置窗口限时 15 分钟，可以重新打开。目录或
凭据报错时，不要为了接通而替换成按量付费端点或其他订阅产品；会前确认套餐可用。

Qoder 的[自定义模型说明](https://docs.qoder.com/cli/custom-models)要求使用向导，
不建议手写 `settings.json` 中的 BYOK 字段。
[Token Plan 概述](https://docs.modelstudio.console.alibabacloud.com/en/model-studio/token-plan-overview)
列出了 Qoder，实际可选项仍以账号界面为准。Key 不进入 Git、Shell 命令或活动截图。

活动使用独立的 Qoder 配置目录，每位参与者结束后重置会保留登录。也可以固定
自定义模型 ID，让每次对比都使用同一个模型。

```bash
export EXPERIENCE_MODEL='your-custom-model-id'
```

Tokenless 仅在活动进程内配置，通过 `--plugin-dir` 加载 `anolisa install tokenless`
附带的原生插件。两次对比加载同一插件，以 `TOKENLESS_COMPRESSION_ENABLED=0/1`
控制压缩，不修改用户全局插件注册。活动之外的日常持久接入可以使用
`anolisa adapter enable tokenless qoder`，然后重启 Qoder。

## 体验一，同一份 Skill，读出 Ubuntu 命令

**展示牌文案**　系统换了，Skill 不用改。

| 时间 | 参与者操作 | 看到的结果 |
| --- | --- | --- |
| 0:00～0:20 | 看脚本展示的原始 Skill | `rpm -ql bash` 与 `dnf install -y tree` |
| 0:20～1:10 | 运行原始工作区 | Qoder 读取原始说明，尝试 RPM 查询 |
| 1:10～2:00 | 用相同提示词运行挂载工作区 | Qoder 读到 `dpkg -L bash` 和 `apt-get` 安装计划 |
| 2:00～2:30 | 对比回答和源文件校验值 | Ubuntu 查询返回 Shell 路径，源文件不变 |
| 2:30～3:00 | 扫描工作人员准备的群二维码 | 邀请带自己的跨发行版 Skill 入群交流 |

```bash
python3 scripts/ubuntu-experience/demo.py skillfs raw
python3 scripts/ubuntu-experience/demo.py skillfs adapted
```

任务只查询已安装文件、展示安装计划，不实际安装 tree。纯净 Ubuntu 通常没有 RPM。
模型也可能自行纠正原始指令，现场应如实说明，不承诺对照组必然失败或耗时更多。
确定可比较的是 **SkillFS 返回的内容，以及源文件不变的 SHA-256**。

配置明确启用 OS 适配并设置 `target_os = "auto"`，关闭 directive stage。普通挂载
通过 `<mount>/skills` 暴露 Skill，再链接到适配工作区的 `.qoder/skills`。每次适配
体验都会启动前台 FUSE worker，完成、中断或超时后卸载，不保留 supervisor。
正式版 0.4.2 尚无 `--read-only` 参数；演示提示词只要求只读操作，并校验原始文件。
适配只作用于 `SKILL.md` 和内置规则允许的匹配，不代表任意脚本或所有 Red Hat 命令
都能自动移植。

## 体验二，更短的 CI 报告，同样找到失败

**展示牌文案**　少读测试日志，照样找出失败。

准备阶段真实运行一组微型 pytest 测试，包含 80 个通过项和一个故意保留的包邮门槛
错误，再把同一份输出放到两个工作区。现场的 `python3 ci_report.py` 成功读取这份
历史报告，报告仍显示 **1 failed, 80 passed**，不表示本次重新执行测试通过。
Tokenless 会保留失败工具的输出，因此直接执行失败的 pytest 命令不适合作为稳定的
压缩展示。

| 时间 | 参与者操作 | 看到的结果 |
| --- | --- | --- |
| 0:00～0:15 | 听任务，定位包邮门槛错误 | 一个具体问题 |
| 0:15～1:00 | 运行 baseline | Qoder 接收完整报告 |
| 1:00～1:45 | 运行 optimized | Qoder 接收保留失败信息的较短报告 |
| 1:45～2:30 | 查看统计并对比回答 | 定位相同失败项，工具内容 Token 估算减少 |
| 2:30～3:00 | 扫群二维码 | 邀请带真实构建或测试输出继续交流 |

```bash
python3 scripts/ubuntu-experience/demo.py tokenless baseline
python3 scripts/ubuntu-experience/demo.py tokenless optimized
python3 scripts/ubuntu-experience/demo.py stats
```

预期诊断为 `test_free_shipping_at_threshold` 在金额 100 时失败，运费实际为 5，
预期为 0。最小修复是把 `shipping_fee` 中的 `<= 100` 改成 `< 100`。计时体验内
不实际改代码。两次运行使用相同样本、提示词、模型选择和输出限制，均为新会话。
每次模型运行最多 90 秒，不重试模型请求；会前用真实账号彩排，确认网络和模型耗时
适合现场节奏。

optimized 必须产生可测的节省量，否则脚本报错。统计是工具内容范围内的估算，不是
完整上下文、耗时收益或 Token Plan 账单。普通汇总会排除 baseline 的 dry-run 记录；
请看 optimized 记录内的 **Before → After**。不承诺固定比例，也不采用模型自报的
Token 数。

## 彩排与换人重置

```bash
# 不需要模型调用或 Key，检查真实 FUSE 读取和已安装的 Qoder hook。
python3 scripts/ubuntu-experience/demo.py check

# 只彩排 Tokenless 本地 hook，不代表真实 Qoder/模型会话。
python3 scripts/ubuntu-experience/demo.py rehearsal

# 每位参与者结束后执行，确保上一轮已退出。
python3 scripts/ubuntu-experience/demo.py reset

# 活动结束，连同活动专用 Qoder 凭据一起删除。
python3 scripts/ubuntu-experience/demo.py cleanup
```

开场前先执行 `check`，再完成 `auth`，实际跑完两组对比。本地 hook 彩排通过不代表
已登录的 Qoder 一定加载了插件；optimized 真实运行后还会检查统计是否记录了节省。
彩排后执行 `reset`，让参与者从空统计开始。

默认目录为 `$HOME/.local/share/anolisa-experience`，权限 0700。`reset` 重建
`runtime/` 内四个工作区、测试报告、回答和每轮数据库，保留 `qoder-config/`、
`skills/`、`source.sha256` 与 `skillfs.toml`。`cleanup` 删除整个活动目录。
已安装的系统包、anolisa/Qoder 二进制和组件留给下次活动使用，不启动监听端口或
常驻演示服务。

所有权标记、进程锁和挂载检查会阻止误删其他目录、正在运行的体验或尚未卸载的
文件系统。原始 Skill 发生变化时，重置会停止并提示检查。启动失败、中断和超时会
停止本轮子进程，保留日志便于定位。若遭遇不可捕获的强制结束，检查
`runtime/mount-process.json` 或 `runtime/qoder-process.json`，核对 PID 和命令后
使用其中记录的停止命令。默认路径还残留挂载时可执行以下操作。

```bash
fusermount3 -u "$HOME/.local/share/anolisa-experience/runtime/mount"
python3 scripts/ubuntu-experience/demo.py reset
```

不要直接删除仍有挂载的目录。专用活动机器退役后，可以按对应范围卸载组件。

```bash
anolisa --install-mode user uninstall tokenless
sudo "$HOME/.local/bin/anolisa" --install-mode system uninstall skillfs
```

更换活动目录时，每条命令都在动作之前加 `--root /absolute/path`。
`--skillfs /path/to/skillfs` 和 `--adapter-dir /path/to/adapters` 用于本地发布验证，
无需覆盖机器已有二进制。

## 策划交接

- Status：脚本已准备，待活动机器及账号彩排；真实 Token Plan 调用需要工作人员
  通过向导登录。
- Started：仅命令运行期间的 FUSE/Qoder 子进程，无端口或常驻服务。
- Changed：活动脚本、样本和本双语指南，没有修改组件代码。
- Validation：在 Ubuntu ARM64 上用 `skillfs/v0.4.2` 标签源码构建 SkillFS，搭配
  官方 Tokenless 0.8.2 ARM64 二进制和 Qoder 1.1.47 插件校验。真实 FUSE 转换和
  源文件校验通过；本地 hook 把样本从 7,349 字符减到 1,369 字符，减少 81.4%，
  保留失败信息。该数字是彩排实测，不是对现场效果的承诺。
  8 项所有权、重置与进程清理测试、Shell 语法、Python 格式、双语目录和文档链接
  检查也已通过。Qoder 实际加载了原生插件及 PostToolUse hook；中断挂载中的体验
  后，真实 FUSE 挂载已卸载，worker 和模拟 Qoder 子进程均已回收。
- Cleanup/remaining：活动机器还需执行 x86_64 安装、账号登录和两组真实会话。
  换人用 `reset`，收场用 `cleanup`，完整命令见上文。
