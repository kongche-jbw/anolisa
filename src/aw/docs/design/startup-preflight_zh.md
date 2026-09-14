# 启动依赖验收

[English](startup-preflight.md)

预检在启动方申请会话资源前验收显式原生依赖，不包含Agent PID、runtime generation、Journal
或hook配置。依赖就绪与真实Agent兼容、执行准入及最终dispatch权限分别记录。

## 组合与归属

现有 `aw-hook-cli` crate负责配置及CLI。`inspect` 只构造SecHost，`project` 依次构造SecHost
和TokenlessHost。Host继续持有各自版本/profile常量、pin、环境控制和共享有界进程runner。
错误保留对应组件及静态诊断，不序列化原生输出。两个启动原生进程的CLI复用进程级信号处理，
库接收调用方持有的取消状态。不新增crate、调度器或包解析器。

安装owner提供已安装的绝对路径。原生版本与operator的Provider release标识保持分离。
预检不根据PATH中发现的程序自动登记预期摘要，不修改安装文件。成功探针不能绕过执行时的
pin和身份检查。

## 可选Herdr获取

fetch保留已审阅、未修改的v0.9.0制品pin。在本次持有的同级staging目录下载并校验二进制和
许可证，再通过Linux原子no-replace rename发布完整bundle，避免历史先发布二进制和chmod
已有文件的行为。已有匹配bundle只读检查，冲突明确报错。目录发布是提交点，此后中断可能
留下完整bundle，不会留下部分发布的文件对。SIGKILL/断电无法执行staging清理，这不构成
崩溃恢复或文件系统持久性证明。

展示bundle与Provider验收独立。两者均不启动Agent或安装hooks。后续launcher仍需解析shell
helper及Agent，绑定真实session/runtime身份并验证插件共存。扩展配置generation不能替代
Agent incarnation、reset边界或pane身份。

## 验证

统一门禁运行真实本地version peer的预检集成测试与离线fetch测试，覆盖按模式选择依赖、
明确版本/pin失败、取消/超时进程回收、原生输出不泄露、制品/许可证失败、不覆盖、目标竞争
及staging清理。真实制品获取作为显式网络验收另记；测试通过不认证已登录Agent或全部架构。

就绪JSON复用已有五秒有界、可取消的stdout交付；不消费输出的管道不会让预检无限等待。
