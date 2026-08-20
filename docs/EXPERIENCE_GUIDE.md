# Reactor 体验指南

## 安装与启动

- 直接运行：`target/release/bundle/macos/Reactor.app`
- 安装镜像：`target/release/bundle/dmg/Reactor_0.1.0_aarch64.dmg`

Reactor 自带受管 Maestro 与所需 Java 运行时，用户不需要另外安装全局 Maestro。
首次执行真实设备任务时，在“设置”或“运行环境”中运行一次工具准备即可。

## AI Flow 生成、预览与修改

1. 打开 **Flow Studio**，输入测试目标、应用包名、框架和平台。
2. 选择 Flow 生成器：
   - **Local Model**：连接已运行的 Ollama、LM Studio 或兼容本地服务，无需云端 Key。
   - **Codex CLI / Claude Code CLI**：复用本机已有安装和登录态，Reactor 不读取凭据。
   - **Cloud API**：只有显式选择并提供当前会话 Key 或钥匙串 Key 后才能调用。
3. 对任意 App，先点击“读取当前界面”并确认本次允许提供脱敏 UI 文本，再使用
   **AI 探索并试跑**。Reactor 会先真实点击一个安全入口、读取新页面，再生成完整 Flow；
   普通“生成”仍适合已经明确知道稳定 Selector 的场景。
4. 生成完成后可切换查看 **步骤 / 完整 Flow JSON / Maestro YAML**。
5. JSON 支持编辑、撤销、复制和应用；应用时 Rust 核心重新校验并重新编译 YAML。
6. 试跑不仅检查 Maestro 是否执行成功，还会比较起始页与目标页 UI 树。目标页验证标记
   必须是导航后新出现的稳定文本或语义 ID，否则拒绝锁定。
7. Reactor 只在非测量阶段进行自愈，试跑成功后由用户确认锁定 Flow。
8. 锁定后才能进入正式测量；任何 Flow 修改都会使旧试跑、锁定和结果失效。

正式测量窗口禁止调用模型，AI 不能改变已经锁定的步骤、数值或回归判定。

## 不连接设备的快速体验

可运行“三框架模拟导览”，体验独立 Runner、任务状态、三框架布局和 HTML 报告。自然语言 Flow 生成必须选择 Local Model、Codex CLI、Claude Code CLI 或 Cloud API，不再用离线关键词模板模拟 AI 结果。

产品导览数据带有 `SIMULATED` 标记，只用于体验工作流，不会进入真实性能结论。

## Android Emulator 测量

1. 在 Android Studio Device Manager 启动 Emulator，并安装待测 Release 应用。
2. 确认 Flow 的包名、文字或 accessibility id 与应用一致。
3. 在 Reactor 刷新运行环境，生成 Flow，执行 Maestro 试跑并确认锁定。
4. 选择“快速验收（1 次 × 5 秒）”或“正式基准（10 次 × 18 秒）”。
5. 正式任务由独立 Worker 执行；关闭 Reactor 后任务继续，重开会恢复同一任务。
6. 在运行记录和结果中心查看 Perfetto、Flashlight、启动、PSS、热状态及 HTML 报告。

Emulator 结果只与同一主机、同一 Emulator 配置的数据比较；不会与物理设备混排。

## iOS Simulator 测量

1. 启动一个 iOS Simulator，并安装待测 Release 应用。
2. 生成 iOS Flow，完成 Maestro 试跑和人工锁定。
3. Reactor 使用独立 Runner 录制并解析 `xctrace` Time Profiler。
4. Simulator 不支持的帧、内存、启动或能耗指标会明确显示“不可用”，不会生成占位值。

iOS Simulator 与 iOS 物理设备始终分组展示。

## 结果分析与 AI 解读

1. 至少完成两次相同 Flow、平台、设备类别和指标定义的运行。
2. 打开 **结果分析**，选择基线与当前运行。
3. Reactor 先执行确定性兼容检查和回归规则，展示数值、阈值、变化比例及原始证据引用。
4. 可再选择 Offline、Local、Codex、Claude 或 Cloud Provider 解释可能原因。

AI 输出分成“已验证事实 / 可能原因 / 建议验证步骤”；未知证据引用会被拒绝，AI 无权改写规则 verdict。

## 统一性能诊断与 RN 深度分析

1. 打开一级入口 **性能诊断**，在顶部选择 React Native、Flutter 或 Lynx。
2. “性能总览”只读取最近 30 个任务中最多 20 个已完成快照；可用 Run 必须有结果、非 `synthetic` 且成功迭代数大于 0。未锁定 Flow 时只称“搜索范围内的可用 Run”，不会声称全局最新。
3. React Native 提供 Render、可疑渲染规则命中、Hermes/JS CPU、时间线/火焰图、未验证统计差异和源码定位视图。Android `memoryPssMb` 显示为“测后 PSS”。
4. 导入 React DevTools Profiler JSON：
   `tests/fixtures/react-profiler-regressed.json`。
5. 可再导入基线：`tests/fixtures/react-profiler-baseline.json`；本地文件始终标记为“导入上下文，Flow 身份未验证”，因此差异不称为回归。
6. 独立导入 `tests/fixtures/hermes-cpu-profile.json` 可同时保留 React Render 与 Hermes/Chrome CPU 热点证据。
7. 要体验 Source Map，导入 `tests/fixtures/hermes-bundle-profile.json`，再导入
   `tests/fixtures/hermes-bundle.js.map`；位置会从 `index.bundle:1:1` 映射到
   `src/screens/CatalogScreen.tsx:1:1`。页面会区分未导入、已加载但 0 个位置可映射和成功映射。

切换 Flow 或框架会清理当前 Profile、Source Map、统计差异和选中 Commit；陈旧异步解析结果不会覆盖新上下文。受管运行时证据缺失时，应返回 Flow 采集，不能用手工 JSON 代替。

Profile、Source Map 和源码位置均在本机 Rust 核心中处理；诊断页不会调用 AI。
Flutter/Lynx 当前共享黑盒性能总览，并明确展示专项采集接入边界，不会用 RN 组件语义生成占位结论。

## 发布加固、诊断与隐私擦除

打开 **设置**，可直接查看数据库 Schema、内置适配器信任范围、AI CLI 超时与输出上限、Profile/Source Map 导入上限和 Trace 磁盘门禁。

- **生成安全诊断包**：只导出版本、能力状态和资源统计，不包含凭据、任务输入、错误正文、绝对路径、截图、UI 树或原始 Trace。
- **擦除截图与 UI 树**：保留运行历史、性能 Trace 和报告，只删除可能含界面内容的证据。
- **清空全部本地测试数据**：两次确认后删除历史、结果、报告、Flow 草稿、诊断包与 Reactor 保存的 Cloud API Key；受管工具保留。

运行任务尚未结束时，Reactor 会拒绝执行擦除。详细边界见 [安全、诊断与资源策略](SECURITY_AND_RESOURCE_POLICY.md)。

## 当前性能指标

- Android：Frame Time P50/P95/P99、Jank、超帧预算、FPS 摘要、冷启动、CPU、PSS、热状态。
- iOS Simulator：xctrace Time Profiler 的采样 CPU、录制时长和原始导出证据；不支持项显式标记。
- 通用证据：逐次迭代、Flow SHA-256、框架、平台、场景、设备类别、OS、采集器版本、构建模式和原始文件哈希。
- RN 诊断：组件 Render/Commit 次数、Total/Self/平均/P50/P95/Max、更新证据、Hermes 热点和源码位置。

FPS 只作为摘要；正式比较优先使用 Frame Time 分布、Jank 和逐次迭代。

## CI 回归门禁

CI 可输出 `analysis.json`、`junit.xml` 和静态 `report.html`，并用退出码区分通过、回归与基线不兼容。完整命令见 [CI_GUIDE.md](CI_GUIDE.md)。

## CLI 等价流程

```sh
cargo build --release -p reactor-cli
./target/release/reactor generate-flow --intent "进入列表并滚动 10 次" --app-id com.example.app --output flow.json
./target/release/reactor trial-flow flow.json trial.json --device emulator-5554 --workspace .
./target/release/reactor lock-flow flow.json flow.lock.json --trial-report trial.json
./target/release/reactor run-android flow.lock.json --framework react-native --scenario list --device emulator-5554 --workspace .
```

不传 `--device` 的试跑只生成产品导览验证证据；这种锁会被真实 Android 性能测量拒绝。
