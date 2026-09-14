# Qoder 单prompt会话归属

[English](qoder-session.md)

启动器围绕一次原生Qoder print-mode prompt组合现有AW二进制。八crate依赖保持不变，
Linux启动编排仅使用Python标准库，不复制Core计划、Provider引擎、历史解析或回执校验。

## 调用与归属

```text
私有operator配置 → 版本/pin、原生协议探针、配置共存检查
    → launcher持有cosh-shell子进程
        → bootstrap记录自身PID/start ticks → exec Qoder -p
            → 原生PostToolUse → 现有AW hook → Core → 原生Host
    → 等待并清理本次子树 → 现有observe → 私有result.json
```

cosh-shell直接命令helper负责启动并等待一个子进程，不是PTY supervisor；外部launcher持有
该根及后代的寿命。bootstrap通过exec保持配置中的Agent进程代次。hook继续验证祖先关系及
runtime/session/tool绑定。每次prompt生成新UUID，不用于reset或下一轮；扩展配置generation
不能替代Agent incarnation。observer不获得Agent终止权。

历史路径由固定Qoder1.1.47 profile和显式配置根派生，原hook验证transcript路径、cwd、session；
launcher不伪造历史前缀。子树退出后每个上下文只observe一次；之后query验证保留的历史快照。
缺证据不等于采用。

## 配置与失败边界

排他新建0700会话目录，配置/身份快照0600。启动准备失败只删除本次资源；尝试启动后保留
诊断状态及显式同意保留的投影证据。原生配置、认证、插件不复制或修改。活跃hooks/plugins、
全局禁用hooks及不支持的设置目录覆盖明确拒绝；已有禁用插件条目保留。尚未认证活跃插件共存。

可执行摘要由operator独立提供；未覆盖的运行库/配置仍归安装owner。文件权限和祖先检查不是
同用户恶意进程隔离。合成PII探针不认证完整检测器、daemon健康、最终授权或sandbox；原生审计保持启用。

Processes只服务专用单线程、无既有子进程且使用默认SIGCHLD回收的调用方，恢复之前的
subreaper及信号处理状态。未回收的leader保留PGID到最后一次组信号；独立组的孤儿Provider
被接管后按精确子PID终止并回收，在有限期限内排空新收养后代。不从持久PID文件恢复停止权，
无关进程不属于此归属边界。

启动工具通过非阻塞管道采集stdout/stderr，每流64KiB实时上限。工具及Agent各有有限deadline；
清理允许TERM1秒、kill/reap3秒。采集初始化或spawn失败也释放资源。内核阻塞或launcher被
SIGKILL不能保证完整清理，不静默宣布成功。

## 验收

统一AW门禁构建现有CLI，再执行非空process/session套件。合成Qoder/cosh peer通过真实AW
协议/身份、投影及独立历史路径，不访问网络或模型。覆盖错会话、缺扫描能力、配置冲突、
禁用hooks、设置来源覆盖、初始化失败、超时取消、根提前退出、跨组后代、无关进程及输出洪泛。

真实Qoder/cosh-shell/SecCore/Tokenless须按具体版本和最终源码另行验收。历史观察仅证明
local_history，模型请求采用与计费另计。本单元不实现交互PTY、多轮/reset、多pane、Herdr UI、
Codex启动或COSH自有dispatcher。
