# Reactor

Reactor 是一款 AI 驱动、证据优先的跨平台移动应用自动化性能测试工具。它使用
Reactor Flow DSL 描述测试步骤，由 Rust 校验并编译成 Maestro YAML；正式测量在独立
Runner 中执行，AI 不进入测量窗口。

当前可体验能力：

- Reactor Offline、Local Model、Codex CLI、Claude Code CLI、Cloud API 五种 Flow Provider。
- Android/iOS Simulator 的 Flow 试跑、失败证据、自愈、人工确认和 SHA-256 锁定。
- Android Emulator 的 Maestro、Flashlight、Perfetto、冷启动、PSS、CPU 与热状态采集。
- iOS Simulator 的 Maestro 与 xctrace Time Profiler。
- 任务历史、结果分析、HTML 报告、CI 回归门禁和 RN 组件 Render/Profile 诊断。
- 数据库升级兼容门禁、资源上限、安全诊断包和本地隐私擦除。
- AI 两阶段探索：先在起始页选择一个安全入口并真实执行，再读取目标页生成完整 Flow。
- 目标页语义证明：目标标记必须在起始页不存在、在导航后的页面存在，否则不能锁定。

## 开发验证

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
pnpm --dir apps/desktop build
pnpm --dir apps/desktop tauri build
```

Node.js/pnpm、Rust、Xcode 和 Android SDK 属于开发依赖。正式 macOS 应用可内置经过哈希
验证的 Maestro、JRE、ADB、Flashlight 和 Perfetto 工具归档，最终用户无需单独安装
Maestro。

## 使用

详细步骤见 [体验指南](docs/EXPERIENCE_GUIDE.md)，架构与里程碑见
[实施计划](docs/IMPLEMENTATION_PLAN.md)。Android Emulator 的结果只允许与同一主机、
同一模拟器配置的结果比较，不与物理设备混排。

## 当前边界

Reactor 已具备通用黑盒探索基础，但不宣称仅凭包名即可完整遍历任意 App。登录、权限、
WebView/Canvas、自绘控件、动态网络内容和多层导航可能需要稳定语义 ID、测试账号、人工
提示或后续的设备镜像/Selector 审查/录制工作台。

外部插件在 0.1 版默认禁用；当前信任边界与资源上限见
[安全、诊断与资源策略](docs/SECURITY_AND_RESOURCE_POLICY.md)。
