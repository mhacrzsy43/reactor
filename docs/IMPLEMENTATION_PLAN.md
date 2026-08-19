# Reactor 实施计划

状态：执行中  
产品定位：AI 驱动、可复现、跨平台的移动应用自动化性能测试工具  
首要目标：统一测试 React Native、Flutter、Lynx，并允许后续接入原生应用和其他框架

## 当前进度（2026-08-19）

| 里程碑 | 状态 | 已验证范围 |
|---|---|---|
| M0 | ✅ 完成 | Reactor 命名、Rust workspace、架构与基准规范 |
| M1 | ✅ 完成 | Flow v1、校验、确定性计划、SHA-256 锁与篡改检测 |
| M2 | ✅ 完成 | 独立 Worker、SQLite 任务/事件/artifact/设备/结果索引、游标、幂等取消、进程组清理、断线重连、孤儿恢复、SHA-256 完整性与防重复执行均已验收 |
| M3 | ✅ 完成 | Rust 受管工具链、固定版本/SHA-256、代理、断点续传、离线缓存、安装锁/原子替换、本地 Maestro override、桌面/命令行 setup、设备能力探测与选择均已验收 |
| M4 | ✅ 完成 | 10 类自然语言 Flow、Android/iOS Simulator 试跑、双平台失败截图/UI 层级、脱敏预览、最多两次 Mock Provider 自愈、diff、人工确认、钥匙串、锁定与测量期 0 模型调用审计均已验收 |
| M5 | ✅ 完成 | Android Emulator 已真实打通 Maestro + Flashlight + Perfetto；帧分位、jank、启动、PSS、热状态、原始 trace、报告、回放与故障处理均已验收 |
| M6 | ✅ 完成 | iOS Simulator 的 Flow 生成、Maestro 试跑、锁定、独立 Runner、xctrace Time Profiler、指标可用性、桌面结果卡和 HTML 报告均已真实验收 |
| M7 | ✅ 完成 | 历史中心、平台分组、任务详情、事件分页、虚拟列表、2Hz 轮询、10 万事件门禁和 Flow 草稿恢复均已完成；正式任务运行中关闭 UI 后 Worker 继续，重开自动恢复同一任务、进度、事件与最终结果的桌面门禁已通过 |
| M8 | ✅ 完成 | 结果分析、确定性回归、RN 组件诊断、Profile Diff、CI 输出与 Release 已完成；Offline、Codex CLI、Claude Code CLI 的 Flow 生成与结果解释真实门禁通过，Local Model/Cloud 为可选配置 |
| M8.9 | ✅ 完成 | 导航 Flow 必须验证最后一次导航；目标标记不能复用入口或只用坐标；试跑后比较起始/目标 UI 树，只有目标页独有标记成立才允许锁定。Android 任务 `75db50f2…` 已通过完整门禁 |
| M8.10 | ✅ 完成（必做） | A–D 全部完成：镜像/Selector、低延迟录制、安全输入、编辑回放、AI 状态图、可见/文本/启用状态断言、目标页唯一性证明、哈希锁定与性能测试衔接均通过 Android Emulator 门禁 |
| M9 | ✅ 完成 | Android Emulator 正式实验 12/12 完成；统一结果与 HTML 报告已生成，模拟器实验集与物理设备结果严格隔离 |
| M10 | ✅ 当前环境必选项完成 | M10.1–M10.5 已完成；macOS `.app`/DMG、数据安全、隐私/资源、稳定更新契约与 1.x 兼容承诺已验收。Windows/Linux/正式签名保留为对应环境或账号条件门禁 |

## 1. 产品原则

1. **AI 负责意图，不污染测量**：AI 生成、解释和修复测试 Flow；正式测量只执行已经校验和锁定的确定性产物。
2. **一次安装**：Maestro、Java、Android Platform Tools 等由 Reactor 管理固定版本，用户不需要单独安装全局 Maestro。
3. **同一核心，多种入口**：桌面端、CLI 和 CI 共享 Rust Core，不出现三套行为。
4. **原始证据优先**：所有结论必须能追溯到原始 trace、设备信息、Flow 哈希和运行日志。
5. **公平比较**：不同框架使用相同设备、场景、数据、测量窗口和统计口径；Android 与 iOS 不合并排名。
6. **本地优先**：默认数据保存在本机。上传截图、UI 树或日志给模型前必须展示范围并执行脱敏。

## 2. AI 能力范围

### 2.0 Benchmark 与 Diagnose 双模式

- **Benchmark 模式**回答“这个版本或框架更快了吗”：使用 Release 构建、黑盒原生采集和锁定 Flow，输出可跨框架比较的 frame time、jank、CPU、内存和启动指标。
- **Diagnose 模式**回答“为什么这里慢”：允许连接框架诊断协议和带符号/Source Map 的诊断构建，展示组件、函数、线程和调用栈，并明确标记诊断开销。
- React Render、Flutter Widget Build、Lynx Component Update 等白盒指标只用于各自框架内部定位，不直接参与 RN/Flutter/Lynx 排名。
- 同一运行不能同时宣称为无侵入 Benchmark 和带探针 Diagnose；结果必须记录模式、探针版本和开销警告。

### 2.1 Flow Copilot

Reactor 统一支持四类真实 AI Provider，用户可按本机条件选择：

1. `Local Model`：连接用户本机的 Ollama、LM Studio 或其他 OpenAI-compatible 本地端点，实现无需云端 API Key 的真实模型体验。
2. `Codex CLI`：调用用户本机已有的 `codex exec`，复用 CLI 已保存的登录态，不读取、复制或保存其凭据。
3. `Claude Code CLI`：调用用户本机已有的非交互命令，复用 CLI 已保存的登录态，不读取、复制或保存其凭据。
4. `Cloud API`：连接 OpenAI-compatible 云端端点，API Key 仅存系统钥匙串。

2026-08-19 决策：删除 Reactor Offline Flow 生成和关键词下一步建议，因为面对任意 App 容易产生格式正确但语义不准确的 Flow。确定性规则只保留在结果判定、校验器和测试夹具中，不再作为 Flow Provider。选择 `Cloud API` 后必须提供 API Key 或明确使用钥匙串中的已保存 Key 才能生成。

OpenAI-compatible 配置接受 Base URL（例如 `https://provider.example/v1`）或完整端点。Base URL 自动尝试 Responses API，再在路由不兼容时回退 Chat Completions；错误必须标注最终请求端点和 HTTP 状态，但清除查询参数并禁止输出 API Key。

本地 CLI Provider 不由 Reactor 静默下载或重新安装；环境检查负责发现版本、登录可用性和结构化输出能力。调用必须发生在测量窗口之外，并使用隔离临时目录、最小权限、固定超时、输出大小限制和 JSON Schema 校验。未安装、未登录、超时或输出不合规时明确报错，并允许用户切换到其他 Provider。

用户用自然语言描述目标，例如：

> 启动应用，进入商品列表，滚动十次，打开第三项详情，然后测量返回列表后的滚动性能。

Reactor 将完成：

1. 解析测试意图和性能目标。
2. 读取应用元数据、可访问性/UI 树和可选截图。
3. 生成框架无关的 Reactor Flow DSL。
4. 静态校验选择器、超时、循环上限、测量边界和敏感操作。
5. 编译为 Maestro Flow 或其他自动化引擎格式。
6. 在非测量模式试跑。
7. 根据失败截图和 UI 树修复不稳定步骤。
8. 生成规范化 JSON，计算 SHA-256，并写入锁定清单。

对于起始页面无法提供目标页标记的黑盒 App，Reactor 不再要求模型一次猜完整路径。AI
探索先生成只包含一个安全入口点击的非测量探针，真实执行后重新读取 UI 树，再基于
“起始页 + 已观察目标页”生成完整 Flow。最终试跑必须证明目标标记在起始页不存在、在
导航后的页面存在；Maestro 退出码为 0 本身不能替代语义证明。

### 2.2 Flow 自愈

- 只在试跑或失败恢复阶段调用 AI。
- 优先使用 accessibility id / semantic id，其次使用稳定文本和层级关系。
- 坐标点击只能作为显式标记的最后手段，并在 UI 中显示风险。
- 每次修复产生结构化 diff、理由和新的 Flow 哈希，未经确认不覆盖已发布 Flow。

### 2.2.1 M8.10 Interactive Flow Explorer（必做）

M8.10 不实现为只能记录坐标和点击序列的传统录制器，而实现为“人工示范 + UI 树语义 + AI 探索 + 确定性验证”结合的交互式 Flow 工作台。Maestro 继续作为 Reactor 管理的执行引擎，但不承担完整的探索、审查和性能测试产品体验。

- **M8.10A：设备观察与 Selector Inspector**：同步 Android Emulator/设备或 iOS Simulator 的画面和 UI 树；点击镜像中的控件即可查看 bounds、文本、accessibility/resource id、Selector 候选、唯一性、稳定性评分和脆弱原因。正式测量期间必须停止同步。
- **M8.10B：操作录制、编辑与回放**：提供互不混淆的“审查模式”和“录制/交互模式”；审查模式点击只选择 Selector，录制模式点击后真实操作设备、自动刷新到下一页面并把语义化步骤加入 Flow，让用户逐页继续选择。输入、滚动、返回和等待同样转换成 Reactor Flow；同步展示步骤、Flow JSON 和实际 Maestro YAML；允许插入、删除、重排、撤销、修改 Selector，并在真实设备上逐步或整体回放。坐标只能作为显式标记的脆弱降级，删除、支付、授权、提交等敏感控件必须二次确认。
  - **普通点击**：点选后先展示即将使用的 Selector；录制模式执行真实点击，等待界面稳定，重新同步画面/UI 树并追加 `tap` 步骤。可撤销最近一步并回放到任意已记录状态。
  - **输入框**：识别可编辑控件后打开 Reactor 输入面板，不依赖镜像键盘盲输；用户选择“追加/清空后输入”，生成语义 Selector + `input_text`，执行后重新读取控件状态。回车、搜索、完成和隐藏键盘作为独立可见动作记录。
  - **动态与敏感输入**：输入值采用显式联合类型 `literal | variableRef | secretRef | promptRef | totpRef`，不再把所有内容塞进普通 `text`。密码、Token 等只把引用名称写入锁定 Flow；Runner 回放时从 macOS Keychain、本机 Secret 或 CI Secret 即时解析，明文只在执行进程内短暂存在，不进入日志、报告、Flow JSON、Maestro YAML 预览、截图说明或 AI 上下文。Flow 哈希锁定引用与解析策略而不是密钥值，允许安全轮换。TOTP 根据本机密钥引用即时生成；短信/邮件验证码使用 `promptRef` 暂停交互试跑，CI 无人值守则必须配置测试验证码服务、固定测试账号或预认证状态。缺少引用时明确暂停/失败，绝不输入占位符冒充成功。
  - **滑动与返回**：镜像手势归一化为方向、相对距离和持续时间，不保存屏幕绝对轨迹；系统返回、键盘返回和页面返回按钮分别记录，避免语义混淆。
  - **系统弹窗与危险操作**：权限、通知、文件选择等系统界面明确标注上下文；授权、删除、支付、提交、外部跳转等动作在执行前二次确认，AI 不得自动批准。
  - **WebView/Canvas/自绘控件**：UI 树无语义时才允许坐标降级，必须显示 Brittle 警告、绑定截图证据并要求真实回放；后续可升级为图像锚点，但不得伪装成稳定 Selector。
  - **动作后同步**：每次交互等待页面进入稳定状态（连续 UI 树/画面或超时策略），再允许选择下一步；超时、键盘遮挡、页面未变化和 App 跳出前台都必须显示为可处理状态，不静默追加错误步骤。
- **M8.10C：AI 状态图探索**：基于已经真实观察到的页面状态生成安全的下一步建议，逐步构建页面/转移状态图；默认不自动触发删除、支付、授权、提交等危险操作；未知或敏感步骤必须由用户确认。不得宣称仅凭包名即可完整遍历任意 App。
- **M8.10D：Assertion Builder 与性能测试衔接**：用户可在目标页点选元素生成可见性、文本或状态断言；最终 Flow 必须通过真实 Maestro 回放，并证明目标标记属于导航后的目标页，才能锁定 Flow 哈希并进入性能测量。

操作录制仍是必做输入方式，但不是唯一生成方式。Reactor 同时支持自然语言生成、人工示范录制、Selector 点选组装、AI 下一步建议和已有 Flow 导入编辑；所有方式最终汇入同一个 Reactor Flow DSL、校验器、真实回放和锁定流程。

M8.10A Android Emulator 桌面验收：Reactor 同步 `emulator-5554` 的 1440×3120 PNG 与 18 个 UI 元素；手动刷新、3 秒低频同步和暂停均生效。点击 RN Demo 的 `List scenario` 时优先命中可交互父控件而不是重复文字子节点，并给出 82/100 Stable 文本 Selector 与 20/100 Brittle 坐标降级。任何非终态任务存在时，后端拒绝截图/UI 树同步。

M8.10B 第一段 Android Emulator 门禁：切换“录制/交互模式”后点击 `List scenario`，Reactor 使用 82/100 文本 Selector 生成单步临时 Maestro Flow；真实执行后进入列表页面，等待 UI 稳定，将 UI 树从 18 个元素刷新为 62 个元素，并在录制时间线追加第 1 个 `tap`。临时执行文件位于 runtime 且执行后删除，不登记为性能证据。联调发现并修复坐标 Selector 的旧 YAML 语法；结构元素不会被一次点击自动执行坐标降级。

M8.10B 镜像交互修正：审查模式选中控件后，Selector 面板始终提供“在设备上点击并继续”，不要求用户回到页面顶部猜测当前模式；执行期间镜像遮罩显示具体动作和页面稳定等待。镜像内滚轮/触控板事件必须阻止 Reactor 页面滚动，并在录制模式转换为设备 `swipe`。Android Emulator 已用受管 Maestro 2.8.0 验证 500ms `UP` swipe 成功。

M8.10B 录制可见性门禁：录制模式右侧持续展示从本次录制开始的完整 Step Flow，而不是只显示最新选中的控件；当前 Selector 与已执行步骤分区展示。点击、返回、滑动及后续输入都必须按实际执行成功的顺序追加，失败步骤不得写入。镜像滚轮在窗口捕获阶段阻止外层页面滚动，并对触控板惯性事件限流。

M8.10B 交互延迟门禁：Android 逐步探索不得为每个点击/返回/滑动重新启动 Maestro/JVM。录制阶段使用当前镜像已审查坐标通过受管 ADB 低延迟执行，同时在 Flow 中保留稳定语义 Selector；动作成功后先返回新截图和追加步骤，再后台刷新较慢的无障碍树。Maestro 继续负责整体回放、目标页语义证明和锁定前最终验证。

M8.10B 安全输入门禁：Flow 输入值已升级为 `literal | variableRef | secretRef | promptRef | totpRef`，旧 `text: string` Flow 与旧锁哈希保持兼容；引用值编译为 `MAESTRO_REACTOR_INPUT_*` 环境占位，Secret/TOTP 密钥保存在独立系统凭据项并由可擦除索引管理，命令失败输出会按解析值脱敏。密码目标拒绝 literal 明文；缺变量、缺 Secret、缺交互值或无效 Base32 均明确失败。RFC6238、旧锁、Inspector EditText、变量/Prompt 缺值和日志脱敏均有自动测试。Android Emulator 使用 `com.android.settings` 的真实 EditText 完成 `promptRef settings.search.once` 试跑：本次值只进入 Maestro 子进程环境，设备显示 Wi‑Fi 搜索结果，Step Flow 只保留 Selector、引用类型和引用名；runtime 未残留临时 YAML 或本次输入值。

M8.10B 编辑与回放门禁：录制工作台提供步骤 / 完整 Flow JSON / Rust 编译后的 Maestro YAML 三视图，支持复制、删除、同 section 重排、一步撤销、JSON 插入或修改 Selector，以及显式选择 setup/measured 边界；跨 section 误拖会被拒绝。新录制默认加入可见的 `launch_app` 起点，避免从未知页面回放。逐步回放不重复写入 Flow；整体回放把 setup/measured/teardown 交给同一 Maestro 进程，Prompt 值必须本次重新输入。Android Emulator 实测 `launch_app → tap List scenario → measured swipe UP`：JSON 经 Rust 校验后生成三段实际 YAML，整体回放成功返回列表并完成滑动，刷新 UI 树为 65 个元素。第一次缺少启动起点的失败被明确显示且未冒充通过，随后据此补齐起点约束。

M8.10C AI 状态图门禁：Flow Explorer 复用 Flow Studio 的 Local Model、Codex CLI、Claude Code 与 Cloud AI Provider 配置；只把脱敏后的可见文本、resource ID、交互属性和 bounds 作为模型上下文，不发送截图、输入值或 Secret。建议默认只展示，危险目标禁止执行，未知目标拒绝盲点，坐标降级要求再次确认。状态图只登记包含真实 UI 元素的页面，并把 Android 低延迟动作产生的空树瞬态延迟到完整 UI 树后再登记转移。早期 Offline 关键词建议门禁已被 2026-08-19 的删除决策取代，不再作为产品能力。

M8.10D Assertion/Performance Handoff 门禁：目标页可点选稳定元素生成“元素可见、文本完全匹配、启用状态与当前一致”三类断言；状态断言编译为 Maestro selector 的 `enabled` 条件，坐标不能作为目标页证明。断言自动放在 measured 边界之前；危险人工操作必须在首次点选后再次明确确认，AI 建议仍不能执行危险动作。Android Emulator 实测从 RN 首页保存 18 元素起始证据，进入列表后选择 `List ready`，形成 `launch_app → tap → assert_visible → measured swipe`；受管 Maestro 整体回放证明标记在起始页不存在、目标页存在（18→59 elements），锁定哈希 `f44177b31775…`。一键交给 Flow Studio 后完成快速真实性能任务 `8f598dd2…`：P95 18.6 ms、Jank 4.1%、冷启动 143 ms、PSS 57.8 MB、CPU 2.7%，原始证据与 HTML 报告已生成。M8.10 完成，下一必选项为 M10.5。

### 2.3 实验设计助手

- 根据测试目标推荐场景、预热次数、正式迭代次数和设备控制项。
- 检查联网、Debug 构建、刷新率变化、热状态、低电量模式等公平性风险。
- 不允许 AI 悄悄改变指标定义或把不同平台结果放入同一个排名。

### 2.4 结果分析助手

- 只读取已归一化指标和可引用的 trace 摘要。
- 输出结论时附带 run id、指标名、样本数和置信信息。
- 区分“数据事实”“可能原因”“建议验证”，禁止把推测写成确定结论。
- 支持询问：哪个框架更稳定、某次回归从哪一步开始、CPU 与卡顿是否相关。
- 支持从异常时间窗口下钻到组件 Render/Build/Update 次数、耗时、调用栈和源码位置。
- 对重复渲染提供规则检测：无输入变化的重复 Render、父组件级联、Context/Props 引用抖动和列表项过度更新；AI 结论必须引用组件名、Commit 和原始 profile。

### 2.5 组件渲染诊断

首期以 React Native 为参考实现，并保留跨框架诊断协议：

1. 受管方式连接 React Native DevTools / React Profiler / Hermes Profile，或导入标准 profile 文件；用户不需要额外全局安装工具。
2. 每个组件记录 Render 次数、Commit 次数、总耗时、Self Time、平均/P50/P95、最大耗时、所属交互和父子关系。
3. 提供按 Render 次数、总耗时和 Self Time 排序的热点组件榜单。
4. 时间线选中异常 Commit 后，可下钻到组件树、JS 调用栈、Source Map 映射后的文件和行号。
5. 重复渲染证据包含前后 Props/State/Context 摘要、触发来源、相邻 Render 间隔和疑似级联路径。
6. 支持两次 profile diff，显示新增 Render、次数变化、耗时变化和新增/消失组件。
7. Flutter 对应 Widget Build / Raster / Timeline；Lynx 对应 Component Update / JS / Native pipeline。三者共享诊断界面和证据模型，但不强行混合语义不同的指标。
8. Profile、Source Map 和源码默认本地处理；发送给 AI 前展示上传范围并脱敏。

Flow Studio 生成后必须自动定位到结果，并同时提供步骤视图、完整 Reactor Flow JSON 和由 Rust 编译器产生的 Maestro YAML；JSON 可直接编辑、撤销和应用，应用前必须由 Rust 校验并重新编译 YAML，且任何修改都必须使旧试跑、锁定和结果失效。概览卡片不能替代可审计、可修改的执行内容。

### 2.6 明确禁止

- 正式测量窗口内调用模型。
- 由模型直接生成最终性能数字。
- 静默上传应用截图、UI 文本、日志或源代码。
- 在没有原始证据时自动宣称性能回归原因。

## 3. 总体架构

```text
React + TypeScript UI (Tauri v2)
                 │ commands / events
                 ▼
reactor-runner 独立 Rust 进程 ───── SQLite 任务与事件日志
                 │
     ┌───────────┼────────────┐
     ▼           ▼            ▼
Flow Compiler  Automation   Collectors
AI Provider    Maestro      Perfetto / xctrace
     │           │            │
     └───────────┴─────┬──────┘
                       ▼
             Normalizer / Statistics
                       │
                       ▼
         原始证据 + 版本化结果 + HTML/桌面报告
```

正式测量由独立 Runner 执行。桌面 UI 可以断开或关闭；任务状态、日志游标和结果均通过 SQLite 恢复。正式运行时 UI 只接收 Runner 持久化的低频观察样本，不持续解析 trace；实时曲线明确标为观察值，最终判定仍使用结束后归一化的正式 artifact。

## 4. Rust Workspace 边界

| 模块 | 责任 | 禁止承担 |
|---|---|---|
| `reactor-protocol` | Flow、Run Plan、Result、插件消息等版本化数据契约 | 进程、网络和 UI |
| `reactor-core` | 校验、编排、状态机、统计、哈希和用例 | 平台命令细节 |
| `reactor-adapters` | 自动化、采集、设备、构建适配器接口与注册表 | 产品 UI |
| `reactor-ai` | Provider 接口、提示输入、结构化输出、脱敏和审计 | 正式测量执行 |
| `reactor-store` | SQLite migration、任务、事件、结果与 artifact 索引 | 大型 trace 二进制本体 |
| `reactor-runner` | 独立进程、任务队列、取消、恢复和进度事件 | 报告页面渲染 |
| `reactor-cli` | setup/doctor/devices/flow/run/report/ci 命令 | 重复实现核心逻辑 |
| `apps/desktop` | Tauri v2、React、Flow Studio、报告和设置 | 直接启动采集器 |

内置适配器使用 Rust trait。第三方适配器采用带版本握手、能力声明和超时控制的 JSON-RPC/stdio 子进程，避免插件崩溃拖垮 Runner。

## 5. 核心数据流

### 5.1 Flow 生成链

```text
自然语言目标
  → AI Draft
  → Reactor Flow Schema 校验
  → UI 选择器解析
  → 安全检查
  → 编译成自动化引擎 Flow
  → 非测量试跑
  → 自动修复/人工确认
  → canonical JSON + SHA-256 锁定
```

锁定清单至少记录：Flow schema 版本、规范化哈希、生成时间、Provider/模型标识、提示模板版本、应用包标识、目标平台、选择器风险、试跑证据和编译器版本。凭据和完整敏感提示不得写入清单。

### 5.2 正式测量链

```text
读取锁定 Flow
  → 环境预检
  → 生成带 seed 的随机运行计划
  → 一次非计分预热
  → 打开原生采集器
  → 执行确定性 Flow
  → 关闭采集器
  → 保存原始证据
  → 归一化和统计
  → 兼容性检查
  → 报告
```

每个阶段写入事件日志。取消操作必须先停止自动化，再安全关闭采集器并保留已产生的原始文件。

## 6. 平台与采集策略

### Android

- 第一阶段保留 Flashlight 适配器，快速复用当前原型。
- 正式版本以 Perfetto/FrameTimeline、`dumpsys gfxinfo` 辅助数据和进程资源采样为主要证据。
- 输出帧耗时分布、jank、CPU、内存、启动时间、热状态和可用时的能耗数据。
- Windows、macOS、Linux 均支持 Android；物理设备是正式比较默认目标。

### iOS

- 仅在 macOS + Xcode 环境启用。
- 直接解析 `xctrace` 导出数据，不使用 Flashlight iOS 的占位 FPS/RAM。
- 模拟器与物理设备结果严格分组；正式跨框架结论默认使用同一物理设备。

### 指标规则

- FPS 仅作为易读摘要，主要比较 frame time、P95/P99、jank 和超预算帧。
- CPU、内存、能耗必须携带定义和采集器版本；定义不兼容时拒绝同列比较。
- 所有报告保留逐次迭代，不只展示一个总分。

## 7. 桌面产品结构

1. **欢迎与环境检查**：自动准备受管工具、显示缺失的系统能力。
2. **项目中心**：应用、包标识、构建产物、平台和团队配置。
3. **Flow Studio**：自然语言输入、结构化步骤、设备画面、UI 树、试跑和 diff。
4. **设备实验室**：设备、OS、刷新率、电量、温度和连接状态。
5. **实验配置**：场景、框架、seed、迭代次数、公平性检查和运行顺序预览。
6. **运行中心**：阶段/Flow 进度、2 秒级 CPU/内存/RN 诊断观察、取消与 UI 断开；不在桌面端执行高频采集或正式判定。
7. **结果中心**：原始迭代、分布、对比、回归基线、AI 解释和证据链接。
8. **设置**：AI Provider、隐私策略、受管工具版本、插件和数据保留。
9. **诊断中心**：时间线、火焰图、Commit/Render 列表、组件热点、重复渲染检测、调用栈、源码定位和 profile diff。

## 8. 实施里程碑

### M0：仓库基线与命名清理

交付：

- 清理 `CrossBench`、旧包名和重复命令。
- 保留 Node 原型为迁移对照，并为其现有行为保留测试。
- 建立 Rust workspace、格式化、lint 和基础测试入口。
- 建立 ADR、实施计划和版本策略。

验收：Node 测试通过；Rust workspace 可构建；全仓没有非兼容用途的旧产品名。

### M1：版本化协议与确定性核心

交付：

- Reactor Flow v1、Run Plan v1、Result v1 Rust 类型和 JSON Schema。
- Flow 静态校验、规范化序列化、SHA-256 锁定清单。
- 将 seeded shuffle、计划哈希、统计函数从 Node 迁移到 Rust。
- golden fixtures 验证 Node/Rust 迁移结果。

验收：相同输入和 seed 始终产生相同顺序和哈希；无效 Flow 提供可定位到步骤的错误。

### M2：Runner 与本地存储

交付：

- `reactor-runner` 独立进程和任务状态机：queued/preflight/warmup/measuring/normalizing/completed/failed/cancelled。
- SQLite migration、任务、阶段、事件、artifact、设备和结果索引。
- 进程重启后的任务恢复、日志游标、幂等取消和 artifact 完整性检查。
- CLI 与 Runner 的本地 IPC。

验收：关闭桌面端不终止任务；Runner 重启后能恢复到可解释状态；同一任务不会被重复执行。

### M3：工具链和自动化适配器

交付：

- 迁移 Java、Maestro、ADB、Flashlight 的固定版本下载与校验。
- 支持代理、断点续传、离线缓存、下载清单和本地 Maestro fork override。
- Android/iOS 设备发现、能力探测和冲突提示。
- Reactor Flow → Maestro 编译器、执行器和 artifact 收集。

验收：新机器只运行 Reactor setup 即可使用受管 Maestro；用户无需全局 Java/Maestro。

### M4：AI Flow MVP

当前已完成：Flow 生成后自动定位到产物，并支持“步骤 / 完整 Flow JSON / Maestro YAML”三视图。JSON 可编辑、取消、复制并应用；应用时由 Rust 再次校验 Flow 并重新编译实际 Maestro YAML，任何修改都会使旧试跑、Flow Lock 和结果失效。2026-08-18 已在桌面应用将重复次数从 10 改为 3，验证步骤视图与 YAML 同步更新。试跑可信性门禁已补齐：未发现 Android Emulator/设备或 iOS Simulator 时，Reactor 只能明确报告“目标不可用”，不得把静态编译称为上机试跑，也不得锁定或用于测量；受管工具解压并识别目标后才可执行真实试跑。

交付：

- Provider-neutral AI 接口，支持云端和本地模型扩展。
- 接入 OpenAI-compatible、Local Model、Codex CLI 与 Claude Code CLI，并保留统一 Provider 扩展接口。
- Provider 能力探测、可执行文件路径选择、版本/登录诊断、结构化输出校验、取消、超时和进程组清理。
- API Key 存入系统钥匙串，不进入配置、日志和数据库。
- 自然语言 → Reactor Flow 结构化生成。
- UI 树/截图上下文裁剪、敏感文本脱敏和上传预览。
- 静态校验、试跑、失败证据、最多限定次数的修复循环。
- Flow diff、人工确认、锁定清单和审计记录。
- Flow 产物三视图、完整内容复制、JSON 编辑、Rust 重校验/YAML 重编译与旧证据失效。

验收：至少 10 个代表性任务中，真实 AI Provider 能生成 schema-valid Flow；失败时不会无限重试；测量阶段审计日志中模型调用数必须为 0。

### M5：Android 原生采集

交付：

- Flashlight 兼容适配器。
- Perfetto 配置、trace 生命周期和解析管线。
- 帧、CPU、内存、启动、热状态的版本化指标定义。
- 异常 trace、设备断连、采集器超时和空间不足处理。

验收：相同 fixture 的解析结果稳定；真实设备连续运行不会泄漏采集进程；每个指标可追溯到原始证据。

### M6：iOS 原生采集

交付：

- xctrace 模板探测、录制、导出和解析。
- 启动、帧、CPU、内存、能耗可用性矩阵。
- 物理机/模拟器、签名、Developer Mode 和 Xcode 版本诊断。
- 对占位或不可比指标硬拒绝。

验收：固定 xctrace fixtures 的解析回归通过；报告不会把模拟器与物理机或不同定义的内存混合比较。

当前提交复验（2026-08-19）：iPhone 15 Pro / iOS 17.5 Simulator 上为 `com.reactor.bench.reactnative` 重新生成并真实试跑 iOS Flow，锁定哈希 `4a73eb567c96…`；Release Runner 任务 `25a02fdc…` 由 xctrace 26.0 取得 22 个 CPU 样本，artifact 完整性通过且测量窗口模型调用为 0。Simulator 不支持的帧、内存、能耗以及缺少 App-ready 专项证据的启动耗时继续返回空值并明确解释，未用占位数字冒充。

### M7：Tauri v2 桌面端

当前已完成：2026-08-18 使用 Android Emulator 快速性能任务 `32e24fc1-16a2-4c75-be30-e33428bfdc48` 完成最终重连门禁。任务在非计分预热阶段关闭 UI 后，独立 Worker 继续运行并由系统接管；重开 Reactor 后自动识别同一 active job，恢复到正式测量阶段，继续接收事件并展示最终性能结果。

交付：

- React + TypeScript 应用框架、语义 Design Token、浅色/深色和响应式布局。
- 环境检查、项目中心、Flow Studio、设备实验室、实验配置、运行中心和结果中心。
- Runner 连接、断线重连、分页事件和大数据虚拟化。
- 报告图表从聚合数据读取，trace 解析留在 Rust 侧。

验收：UI 关闭不影响运行；10 万条事件不会锁死界面；正式测量期间 UI 事件刷新不超过 2Hz，并支持完全断开。

### M8：AI 结果分析与回归检测

当前已完成：Codex CLI 与 Claude Code CLI 可在 Flow Studio 中直接选择；自动发现 macOS 应用包、Homebrew 和 PATH 中的已有安装，复用已有登录态且不读取凭据。两者均使用非交互最小权限模式、隔离临时目录、120 秒超时、1 MiB/256 KiB 输出限制、严格 JSON Schema 与 Reactor 二次校验。2026-08-18 已分别使用本机真实 Codex CLI 和 Claude Code CLI 成功生成有效 Flow，并真实解释同一份不可变回归报告；两者均保留 Rust verdict，事实与假设的证据引用校验通过。

#### M8 子计划与预计剩余时间（2026-08-18）

| 子项 | 状态 | 交付与验收 | 预计剩余 |
|---|---|---|---|
| M8.1 Provider 注册表 | ✅ 完成 | Codex CLI、Claude Code CLI 必过门禁通过；Local Model/Cloud 作为可选 Provider，配置、能力诊断、Schema 与安全测试均完成；Offline Flow 生成已删除 | 0 日 |
| M8.2 证据包与基线兼容 | ✅ 完成 | 版本化 evidence bundle；硬拒绝模拟器/物理机、平台、Flow、指标定义不兼容；原始 trace 引用完整 | 0 日 |
| M8.3 确定性回归规则 | ✅ 完成 | 帧、Jank、启动、CPU、内存、FPS 阈值比较；无 AI 也能判定并引用证据；注入回归测试通过 | 0 日 |
| M8.4 结果分析中心 | ✅ 完成 | 自动推荐兼容基线、任务选择、兼容性提示、指标 diff、回归卡片、证据下钻已通过真实桌面验收 | 0 日 |
| M8.5 AI 结果解释 | ✅ 完成 | 真实 AI Provider 已接入统一接口；事实/推测分区、引用校验、判定不可改写完成；确定性规则总结作为非 AI 工具保留 | 0 日 |
| M8.6 统一性能诊断与 RN 深度分析 | ✅ 完成 | “性能诊断”一级入口、RN/Flutter/Lynx 框架切换、黑盒性能总览；RN 提供 Render、重复渲染、Hermes、时间线/火焰图、Profile Diff、Source Map，并支持指标/规则异常下钻 | 0 日 |
| M8.7 重复渲染与 Profile Diff | ✅ 完成 | 无 AI 规则已定位无变化重复 Render、父组件级联、具体 Commit 与源码；构造基线/回归样例显示 3 个组件回归 | 0 日 |
| M8.8 CI 与最终门禁 | ✅ 完成 | `analysis.json`/JUnit/HTML、退出码 0/2/3、全量测试、clippy、格式检查、Release/DMG 与诊断中心桌面门禁均已通过 | 0 日 |

M8 已完成。Local Model 与 Cloud API 属于可选 Provider；Codex CLI 或 Claude Code CLI 可复用已有登录态。规则分析、诊断和 CI 不依赖 AI。

真实验收：桌面端自动选择同一 Flow 的 Android Emulator 运行 `8bad7514… → f01c9166…`，兼容性检查通过；规则层检测到 P95 帧耗时 +52.2%、Jank +200.5%、冷启动 +22.6%，每条结论均带基线和当前 evidence 引用。不同 Flow 的 `f01c9166… → 32e24fc1…` 被硬拒绝为不兼容。RN Profile 门禁导入 3 个组件、24 次 Render、8 个 Commit，发现 5 条重复渲染/级联规则，并在 Diff 中定位 CatalogScreen、ProductList、ProductCard 三项回归；Hermes Profile 与 Source Map 在正式 Release 中把 `index.bundle:1:1` 映射到 `src/screens/CatalogScreen.tsx:1:1`。真实 Codex CLI 与真实 Claude Code CLI 的 Flow 生成/结果解释门禁均已通过，规则 verdict 保持不变，未知 evidence 引用会被拒绝。

交付：

- 基线选择、兼容性检查、分布比较和回归阈值。
- AI 基于结构化 evidence bundle 解释数据。
- Flow 生成支持 Local Model、Codex CLI、Claude Code CLI 或 Cloud API；结果中心另保留明确标注为非 AI 的确定性规则总结。
- 接入 Local Model、Codex CLI 和 Claude Code CLI Provider；复用用户已有安装/登录态，并完成能力探测、可执行文件路径选择、版本/登录诊断、结构化输出校验、取消、超时和进程组清理。
- 每条结论附指标引用；推测与事实分区。
- CI 生成机器可读退出码、JSON/JUnit 摘要和静态 HTML。
- 建立版本化 Diagnostic Profile 协议，区分 RN Render、Flutter Build 和 Lynx Update 语义。
- React Native 首期接入 React Profiler/Hermes profile：时间线、火焰图、组件 Render 次数、Commit、Self Time、调用栈和 Source Map 定位。
- 重复渲染规则层在不调用 AI 时也能输出组件、次数、对比窗口和证据；AI 负责解释可能原因并提出验证步骤。
- 支持 profile 基线与 diff，把组件 Render 次数或耗时回归纳入 CI，但不与黑盒帧指标混为同一判定。

验收：故意注入的性能回归能被规则层独立发现；关闭 AI 后所有数字和判定仍然成立；构造的重复渲染样例能定位到具体组件、Commit、Render 次数和源码位置。

### M9：三框架等价 Demo 与真实对比

交付：

- React Native、Flutter、Lynx 的 startup/list/update/animation release 应用。
- 相同数据 seed、项目数量、更新频率、动画对象、可访问性标识和完成标记。
- Android 和 iOS 分开的实验集与公开方法说明。
- 真实设备重复测试、原始迭代和离群点说明。

验收：自动检查场景参数等价；每个框架完成预热 + 默认 10 次正式迭代；报告可从原始 artifact 完整重建。

证据复核（2026-08-19）：Android Emulator 的 RN/Flutter/Lynx × startup/list/update/animation 共 12 个非 synthetic 结果仍能定位到各自原始 Flashlight、Perfetto、原生指标、Flow 与报告；当前 Release CLI 从仓库之外的工作目录逐项复算 12/12 artifact 均通过。复核同时修复了旧相对 artifact 路径错误依赖进程当前目录的问题；新记录统一索引绝对路径，旧记录按所属 workspace 解析。物理设备实验仍保持“未执行/后置”，不与这组模拟器结果混排。

### M10：跨平台发布与加固

#### M10 子计划与平台门禁

| 子项 | 必选性 | 当前状态 | 完成口径 |
|---|---|---|---|
| M10.1 macOS 可体验版 | 必选 | ✅ 完成 | 最新 `.app`/DMG 在隔离工作区启动；独立导览任务、历史、报告入口和重启后历史恢复通过桌面验收 |
| M10.2 升级与数据安全 | 必选 | ✅ 完成 | v1→v2 保留历史；迁移失败事务回滚；未来版本数据库拒绝覆盖；严格测试通过 |
| M10.3 诊断与隐私 | 必选 | ✅ 完成 | 安全诊断包默认无凭据/任务正文/路径/截图/UI 树；敏感 artifact 与全部本地数据擦除入口完成 |
| M10.4 插件与资源加固 | 必选 | ✅ 完成 | 仅启用内置可信适配器，外部插件默认禁用；CLI 超时/输出、Profile/Source Map 大小和 Trace 磁盘门禁公开且强制 |
| M10.5 更新与稳定版承诺 | 必选 | ✅ 完成 | Stable/Beta 检查、受限流式下载、安全暂存、App/数据库备份、Helper 原子切换、候选版本健康探针和失败自动回滚已实现；Ed25519 发布工作流与 1.x 兼容承诺通过门禁 |
| M10-Windows | 可选平台 | ⬜ 待对应环境 | 在真实 Windows 环境验证安装包、签名、Android 工具链及 Codex/Claude CLI；缺少 Windows 不阻塞当前版本 |
| M10-Linux | 可选平台 | ⬜ 待对应环境 | 在真实 Linux 环境验证发行包、桌面集成、Android 工具链及 Codex/Claude CLI；缺少 Linux 不阻塞当前版本 |
| M10-Signing | 可选账号门禁 | ⬜ 待账号 | macOS notarization 与 Windows 正式签名只在具备相应开发者账号时执行，不以临时或伪造签名冒充通过 |

Windows/Linux 与签名门禁采用和 Local Model/Cloud Provider 相同的条件验收口径：产品保留实现与诊断入口，环境缺失时显示“未执行”，不阻塞当前 macOS 可体验版完成，也不宣称对应平台已经通过真实验收。

M10.1–M10.4 真实验收（2026-08-18）：最新 Release 在 `/tmp/reactor-m10-qa.Tu83E4` 全新工作区启动；设置页显示数据库 Schema v2、插件契约 v1、120 秒 AI CLI 超时、1 MiB/256 KiB 输出上限、64 MiB Profile、128 MiB Source Map 与 128 MiB Trace 磁盘门禁。桌面生成安全诊断包后复核 JSON，不含绝对路径、凭据、任务输入、错误正文、截图或 UI 树。独立 Worker 完成导览任务 `82b407e9-ae77-4ed5-9353-9f01b938c658`，历史页展示 6 条事件、三框架明确 `SIMULATED` 结果和报告入口；退出并重开同一 Release 后任务、事件和结果仍完整恢复。数据库升级、失败回滚、未来 Schema 拒绝、隐私擦除和导入上限均有 Rust 测试覆盖。

M10.5 执行门禁（2026-08-19）：设置页可显式选择 Stable/Beta，开发构建没有生产公钥时拒绝下载或安装。正式链路先验证 Ed25519 manifest、协议兼容、平台/架构和版本，再以有界流式下载校验大小/SHA-256；安全解包限制为 2 GiB/10 万文件并拒绝链接与越界路径。候选 App 复制到当前安装同卷，Helper 等待桌面退出后备份 App 与 SQLite/WAL/SHM、原子切换，并由候选二进制检查精确版本、数据库/历史和受管工具声明；失败恢复旧 App/数据库，最近事务状态回显在设置页。更新单元/故障测试 16 项、全仓测试、严格 clippy、前端生产构建、真实候选健康探针正反门禁与临时 Ed25519 manifest 生成均通过。`.github/workflows/release.yml` 强制公私钥匹配后才构建和签名，Beta prerelease 不污染 Stable latest。Apple notarization、Windows 签名及 Windows/Linux 真环境仍是条件门禁，不被本项冒充完成。

最终 macOS 可体验版复验（2026-08-19）：全仓 Release 测试与严格 clippy 通过；最新 `Reactor.app` 和 `Reactor_0.1.0_aarch64.dmg` 已重新生成。整个 App 使用 identity `-` 做 ad-hoc codesign，源码目录 App 与只读挂载 DMG 内的 App 均通过 `codesign --verify --deep --strict`，DMG 自身校验有效，SHA-256 为 `c5a9f0a3f390812a21288a85f81312fe22bf08c0a109353d3c72bb7bda90d664`。打包 App 的候选版本健康探针通过；桌面实测可切换 Stable/Beta，开发构建点击检查时明确拒绝无发布公钥的下载/安装。ad-hoc 不冒充 Developer ID/notarization，后两项继续标记待账号。

交付：

- macOS/Windows/Linux 安装包；macOS 签名与 notarization，Windows 签名。
- 自动更新、Schema migration、崩溃恢复、诊断包和隐私擦除。
- 插件能力清单、签名/信任策略、资源上限和兼容性测试。
- Codex CLI/Claude Code CLI 跨平台发现、兼容版本矩阵和升级诊断；Reactor 不捆绑、不静默安装第三方 AI CLI，也不导出其登录凭据。
- 文档、示例项目和首个稳定版升级承诺。

验收：干净系统安装测试通过；升级不丢历史；诊断包默认不含凭据和未授权截图。

### M11：Flow 驱动的验证闭环与内存泄漏检测

目标：用同一份已锁定 Flow 完成“基线 → 故障版回归 → 性能下钻 → 整改 → 复测 → CI”闭环，证明 Reactor 解决的是 AI 代码产出之后的可信验证问题，而不是只生成自动化脚本。

| 子项 | 状态 | 完成口径 |
|---|---|---|
| M11.1 验收协议 | ✅ 完成 | 固定相同 App ID、设备、Release 构建、Flow Hash 和采集器版本；正常版与故障版只允许改变被验证实现 |
| M11.2 Soak/Leak Run Plan | ✅ 完成 | setup 一次、同一进程执行 N 个行为循环、按固定轮次采样、cool-down 后再采样、teardown 一次 |
| M11.3 内存证据与判定 | ✅ 完成 | 保存逐检查点 PSS/RSS/Java Heap/Native Heap、CPU、增长斜率、单调增长比例、首尾差和冷却回落；只凭趋势标记“疑似泄漏”，有对象保留证据才允许“确认泄漏” |
| M11.4 实时性能观察 | ✅ 完成 | Flow 主测量与 Soak 均低频展示进度、CPU、PSS/RSS、Java/Native Heap、RN Tree/Profile、Console/Network 和趋势；观察值不进入最终 Benchmark 判定，断开 UI 不影响 Runner |
| M11.5 RN 受管诊断 | ✅ 完成 | SDK 本地桥、真实 React Fiber 组件树、Console/Network、Profiling Renderer、Flow 自动 Profile、Hermes JS Heap Snapshot、Java HPROF、JSI Heap Stats 与 Perfetto heapprofd 均已在 Android Emulator 真实采集 |
| M11.6 故障 Demo | ✅ 完成 | 正常、重复渲染和内存保留三种确定性 APK 已用相同 Flow、App ID、设备和采集器完成真实对照；趋势与对象保留证据分级判定通过 |
| M11.7 整改故事与 CI | ✅ 完成 | 故障版由规则检出并下钻到组件/源码，整改后同 Flow 复测恢复；CI 真实先以退出码 2 失败、再以退出码 0 通过，结论可从 artifact 重建 |
| M11.8 模拟器验收与交付 | ✅ 完成 | Android Emulator 全流程证据、全仓测试、严格 clippy、桌面生产构建、App/DMG、签名校验、正式安装与界面验收均已完成 |

Soak/Leak 的采样策略属于 Run Plan，不写入 Flow DSL。Flow 只描述用户行为；这样同一 Flow 可在普通 Benchmark、长期稳定性和内存泄漏测试中复用，且修改采样频率不会改变行为哈希。

首个故事使用四段演示：AI/录制生成并锁定登录与列表 Flow；三框架等价黑盒基准；RN 重复渲染 Profile Diff；正常版/泄漏版在同一进程循环后的内存趋势对比。用户已确认实时组件树、Console/Network、Flow 自动 Profile 与堆级证据也属于本轮必做能力；对未接 Reactor RN SDK 或非 profileable 的第三方 App 必须显示能力缺失，不能生成占位证据。

M11.1–M11.3 首次真实门禁（2026-08-19）：Android Emulator `emulator-5554`、RN Release、Flow Hash `a6fff11504bf…` 完成任务 `6307b013-1834-44ee-a1fd-3d23f9dc4a3a`。setup 只执行一次，随后同一进程循环 6 轮并在第 2/4/6 轮与 cool-down 后采样；PSS 为 58.08/65.85/66.60 MB，冷却回落 1.50 MB。由于 warm-up 后只有两个有效趋势点，规则正确返回 `insufficient_evidence`，没有把短样本冒充泄漏。任务保存独立 `android-memory-leak.json`、逐轮 SQLite 遥测事件、Perfetto、Flashlight 和最终 HTML；状态机严格保持 Measuring → Normalizing，禁止逆向阶段迁移。

M11.4 与 M11.5 第一段真实门禁（2026-08-19）：任务 `dcd306cc-60a2-4706-b4f9-f0bb3609f63f` 在 Android Emulator 上用相同 Flow Hash 执行 Profiling Release。运行期间 SQLite 每 2 秒出现 `live_telemetry`，实测可同时看到 PSS 61.93 MB、Java/Native Heap 5.23/15.57 MB、16 次 Fiber Tree Commit、4 次 Profile Commit 与 Console 计数；结束后保存 27 条 RN 本地事件、最新 24 节点真实组件树和自动生成的 `rn-profile.json`。最终正式帧/CPU/内存仍来自 Flashlight、Perfetto 与结束后归一化，实时观察样本明确不参与 verdict。

M11.5 完整堆证据门禁（2026-08-20）：诊断构建将 `react-android`、旧 `react-native:+` 与 `hermes-android` 坐标统一替换为 RN 0.87 源码工程，并以 `HERMES_MEMORY_INSTRUMENTATION` 编译专用 Hermes；正式 Release 仍使用官方预编译 Hermes。ARM64 Diagnostic APK 安装到 `emulator-5554` 后，登录进入 Memory scenario 并真实执行 6 个循环，页面显示 `Memory cycle 6 complete`，最终保存 2.8 MB `rn-hermes.heapsnapshot`、38 MB `rn-java.hprof`、4.9 KB JSI Heap Stats、56 KB React Profile 与 46 KB RN 事件流，且无 Heap Snapshot 错误或进程崩溃。

M11.6 三版故障门禁（2026-08-20）：固定 Flow Hash `a6fff11504bf…`，同一 `emulator-5554` 与 RN Release 分别完成 normal `f8f2b208-a580-42f1-8d27-b873ee49a1ff`、duplicate-render-fault `750088de-0eb2-4fed-9661-4a0a12608017` 和 memory-retention-fault `3ec4ab29-9f8f-432d-801d-1d5700fd4282`。重复渲染版 Render 44 次（正常版 8 次），内存保留版在增长趋势成立的同时记录 6 个 RN 保留对象 / 6,291,456 bytes 与 104,080 native retained bytes，判定为 `confirmed_leak`；正常版保持 `insufficient_evidence`，未误报泄漏。

M11.7 整改与 CI 门禁（2026-08-20）：同一 scenario `m11-source-regression`、Flow Hash 和设备下，正常基线 `383baf59-d27b-4e34-bc5f-6defb9cc2195` 对重复渲染故障 `b8ddb9ef-07f2-4c63-9062-2102e3a69bab` 的 CI 退出码为 2；`MemoryScenario` Render 2→8（+300%），并准确下钻至 `demos/react-native/App.tsx:170`。恢复正常实现后的 Run `99732f06-5b29-4649-a947-501562f5e66b` 对相同基线退出码为 0，Profile 回归数为 0。两轮均从保存的 result/Profile artifact 重建 `analysis.json`、`junit.xml`、`report.html`，JUnit XML 校验通过。为避免亚毫秒组件耗时的相对比例噪声误阻断 CI，耗时回归新增 5ms 绝对变化下限；Render 次数回归规则保持不变。

M11.8 最终交付门禁（2026-08-20）：全仓 125 个非忽略测试通过，受管 Trace Processor ignored 真机依赖测试单独通过，严格 clippy 零告警，桌面 TypeScript/Vite 生产构建通过。最终 arm64 `Reactor.app` 与 `Reactor_0.1.0_aarch64.dmg` 成功生成，App 通过 deep/strict codesign 校验；DMG SHA-256 为 `1f5c4ff600f53ee25e69f4e45a72b84d3213cc98c0a78249fe56513ec582634c`。新版已安装到 `/Applications/Reactor.app`，前版可从 `/Applications/Reactor.pre-m11-20260820.app.disabled` 恢复。正式安装版界面验收确认性能总览、实时组件/日志、24 节点组件树、16 次 Tree Commit、4 次 Profile Commit、Console、Hermes Heap 与对象生命周期时间线均可访问；未配置 Apple 开发者凭据，因此仅作 ad-hoc 签名、未 notarize。

## 9. 测试体系

- **单元测试**：DSL 校验、统计、hash、状态机和脱敏。
- **属性测试**：随机计划覆盖、取消幂等、Schema round-trip。
- **Golden 测试**：Maestro 输出、标准结果、HTML 摘要和 AI 结构化响应。
- **契约测试**：内置/外部适配器、Provider 和 IPC 版本握手。
- **回放测试**：不连接设备即可用保存的 Perfetto/xctrace fixture 重建结果。
- **真实设备测试**：Android/iOS 各维护至少一个固定设备基线。
- **故障注入**：断线、超时、低磁盘、坏 trace、UI 退出、Runner 重启和模型限流。
- **自身开销测试**：记录 Runner 空闲/测量开销，比较 UI 连接、低频连接和完全断开三种模式。
- **诊断 fixture**：保存脱敏的 React Profiler/Hermes、Flutter Timeline 和 Lynx profile，验证火焰图、组件 Render 次数、Source Map、重复渲染规则和 profile diff。

## 10. 发布门槛

首个可用版本必须同时满足：

1. AI 可以生成、试跑、修复并锁定 Flow。
2. 正式测量窗口没有模型调用和非必要网络访问。
3. Android 指标来自可验证的原生证据；iOS 不输出占位数据。
4. CLI、桌面端执行相同 Flow 得到相同计划哈希。
5. UI 退出不影响 Runner，结果可以从原始 artifact 重建。
6. RN/Flutter/Lynx 的等价场景通过自动与人工审查。
7. 报告明确展示设备、OS、构建模式、采集器、Flow hash、原始迭代和警告。

## 11. 当前执行顺序与里程碑门禁

里程碑按 M2 → M3 → M4 → M5 严格验收。后续阶段可以有用于降低技术风险的探针代码，但只标记为“预研资产”，不计入该里程碑进度，也不能绕过前置验收。

1. **M2 Runner 闭环**：完成重连、日志游标、幂等取消、子进程组清理、Worker 崩溃恢复和 artifact 完整性检查；通过关闭 UI、取消、强制杀 Worker 三类验收后才进入 M3。
2. **M3 受管工具链闭环**：Rust 下载器、SHA-256、代理、断点续传、离线缓存、固定版本和本地 Maestro fork override；在无全局 Java/Maestro 的环境验收后才进入 M4。
3. **M4 AI Flow 闭环**：UI 树/截图证据、脱敏与上传预览、限次自愈、Flow diff、人工确认、钥匙串和审计；通过代表性用例且确认测量期间模型调用数为 0 后才进入 M5。
4. **M5 Android 原生采集**：将现有 Flashlight 探针纳入正式适配器，再完成 Perfetto、指标定义、异常处理和原始证据追溯。
5. **M6–M10**：iOS xctrace → 桌面产品闭环 → AI 分析与诊断中心 → 三框架等价 Demo → 发布加固。

模拟器数据仅用于同一主机和同一模拟器配置的开发回归；物理设备阶段继续保留，且两类结果永不混排。

### M5 验收记录（2026-08-18）

- 固定 Perfetto trace 回放得到 498 帧、P95 20.140619 ms、P99 45.129122 ms 和 98 个 jank 帧；损坏 trace 被拒绝。
- 低空间、设备断连、采集器超时和进程组清理均有自动化回归测试。
- Reactor 桌面端完成自然语言 Flow → Android Emulator 试跑 → 人工确认锁定 → 独立 Runner 原生采集 → 结果卡 → HTML 报告闭环。
- 桌面验收 Run ID：`a3be9b63-5ed7-4119-b88c-d7258d284a0f`；173 帧、P95 18.590783 ms、P99 34.738998 ms、Jank 2.8902%、冷启动 186 ms、PSS 72.1992 MiB、热状态 0 → 0。
- artifact 完整性检查为 0 个问题，测量窗口模型调用数为 0，任务结束后无残留 Runner、Flashlight、Maestro 或 Perfetto 采集进程。
- 桌面端提供“快速验收（1 次 × 5 秒）”与“正式基准（10 次 × 18 秒）”预设；Flashlight 超时预算单独包含每次 Maestro 自动化开销。

### M6 验收记录（2026-08-18）

- React Native iOS Release Simulator App 已完成 CocoaPods、Release 构建并安装到 iPhone 15 Pro Simulator（iOS 17.5），界面完成标记为 `Reactor ready`。
- Reactor 桌面端完成自然语言 Flow → iOS Simulator Maestro 试跑 → 人工确认锁定 → 独立 Runner xctrace → 结果卡 → HTML 报告闭环；平台切换使用可键盘/辅助操作的 Android/iOS 双按钮，并正确展示 Simulator OS 版本。
- 桌面验收 Run ID：`b8305250-1b81-47d2-8690-4988b87c8c43`；Flow SHA-256 为 `5e7c8cfea5350d6c0df86e869f724bab61cb37597d99a339477957b1bf0fd423`，xctrace 26.0 (17C529)，68 个 CPU Running 样本，采样 CPU 0.919942%，录制时长 7391.769 ms。
- iOS Simulator 帧、内存和能耗明确标记不支持；启动指标因缺少 app-ready 原生证据标记为未宣称，没有输出占位值，也没有与物理设备结果混排。
- 10 个登记 artifact 的 SHA-256 全部复核一致，测量窗口模型调用数为 0，任务结束后无残留 Runner、Maestro、xctrace 或 XCTest 进程。
- `cargo test --workspace --all-targets` 全部非忽略测试通过；受管 Trace Processor 回放测试单独通过；`cargo clippy --workspace --all-targets -- -D warnings` 零告警；前端生产构建与最终 `Reactor.app` 重建成功。
