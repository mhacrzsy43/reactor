# Reactor 安全、诊断与资源策略

本文记录 Reactor 0.1 桌面版实际强制执行的发布边界。界面中的“设置 → 发布加固与资源策略”读取同一组 Rust 常量，不以文案替代约束。

## 适配器与插件信任

- 当前只启用随 Reactor 构建、版本固定的 `maestro`、`android-perfetto`、`android-flashlight` 和 `ios-xctrace` 内置适配器。
- 外部插件默认禁用。插件契约版本为 v1；未来开放安装前，必须增加签名或用户显式信任、能力声明、协议握手与兼容版本检查。
- Reactor 不下载 Codex CLI 或 Claude Code CLI，也不读取或导出它们的登录凭据；只在非测量阶段调用用户已经安装并登录的 CLI。

## 已强制的资源上限

| 范围 | 上限或门禁 |
|---|---|
| Codex/Claude CLI | 120 秒超时；stdout 1 MiB；stderr 256 KiB；超限拒绝结果；超时终止进程组 |
| React/Hermes Profile | 64 MiB |
| Source Map | 128 MiB |
| Android 本地 Trace 空间 | 开始采集前至少 128 MiB 可用 |
| Android 设备 Trace 空间 | 开始采集前至少 64 MiB 可用 |
| Flow wait | 单步超时受 Flow Schema 上限约束 |
| 外部插件 | 禁用，因而不能启动未受信任插件进程 |

受管 Maestro、Perfetto、Flashlight 和 xctrace 命令均有有限超时；超时时 Reactor 清理对应进程或进程组。正式测量窗口禁止 AI Provider 调用。

## 数据库升级

- SQLite 使用版本化、事务式 migration。
- 从 v1 升级到当前 v2 时保留任务和事件历史。
- migration 中任一步失败时事务回滚，旧历史和 `user_version` 保持不变。
- 若数据库来自比当前 Reactor 更新的版本，旧版直接拒绝打开，不会尝试降级或覆盖。

## 安全诊断包

“生成安全诊断包”只包含版本、平台、数据库版本、历史数量、磁盘统计、无路径的工具可用性和资源策略。默认明确排除：

- API Key 和任何凭据值；
- 任务输入、Flow 内容与错误正文；
- 用户目录和其他绝对路径；
- 截图、UI 树及其内容；
- 原始性能 Trace。

诊断包写入 Reactor 本地数据目录下的 `.reactor/diagnostics/`，只有用户主动分享时才会离开本机。

## 隐私擦除

- “擦除截图与 UI 树”只删除试跑证据中的截图和 UI 层级文件，保留性能 Trace、报告和运行历史。
- “清空全部本地测试数据”需要两次确认，并删除运行历史、结果、报告、Flow 草稿、诊断包和 Reactor 保存的 Cloud API Key；受管工具缓存保留，避免下次启动重新下载。
- 存在运行中的任务时，任何擦除操作都会被拒绝。
