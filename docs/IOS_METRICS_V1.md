# Reactor iOS 原生指标 v1

状态：`ios-native-v1`（iOS Simulator 阶段）  
采集器：`xctrace-time-profiler-v1`  
当前验证环境：Xcode 26.3 / xctrace 26.0、iPhone 15 Pro Simulator / iOS 17.5

## 原则

- iOS Simulator 与物理设备永不混排。
- 只输出 xctrace 原始证据能够支持的数字；不可用指标保留结构化 availability，不填 `0` 或占位 FPS。
- `.trace` bundle 会归档为可校验的 ZIP，同时保留 TOC 和 Time Profiler XML 导出，报告中的数字可从导出文件重建。
- 正式采集前必须在同一 Simulator 上完成 Flow 试跑并锁定 SHA-256；测量窗口不连接 AI Provider。

## Simulator 可用性矩阵

| 指标 | 状态 | v1 证据与口径 |
|---|---|---|
| CPU | 可用 | xctrace `Time Profiler` 的 Running sample weight 总和 ÷ 录制时长；可能超过 100%，表示多核总 CPU |
| 帧 / Hitches | 不支持 | Xcode 26.3 在该 Simulator 返回 `Hitches is not supported on this platform`，因此不输出帧数字 |
| 启动 | 未宣称 | `App Launch` 能录制进程启动，但没有应用 ready/TTI 证据；v1 不把命令耗时冒充启动性能 |
| 内存 | 不支持 | `Activity Monitor` 在该 Simulator 返回 `Activity monitoring service not available on this device` |
| 能耗 | 不支持 | Simulator 不能产生可与物理设备比较的能耗结论 |

物理设备的帧、启动、内存和能耗能力必须在后续真机门禁中逐项验证；在此之前均不得从 Simulator 值外推。

## CPU 计算

```text
cpu_mean_pct = sum(running_sample_weight_ns)
               / recording_duration_ns
               × 100
```

Time Profiler XML 中 `<weight id>` 与 `<weight ref>` 都计为一个 Running sample。解析器必须解析引用并拒绝缺失 duration、非法 weight 或未解析引用。

## 每次运行保留的证据

- `time-profiler.trace/`：原始 xctrace bundle
- `time-profiler.trace.zip`：纳入 Reactor artifact SHA-256 校验的原始 bundle 归档
- `xctrace-toc.xml`：设备、OS、模板、工具版本和录制时长
- `xctrace-time-profile.xml`：Time Profiler 表导出
- `ios-native-metrics.json`：版本化指标和 availability
- `ai-audit.json`：测量窗口模型调用数必须为 0
- `result.json` 与 `report.html`：归一化结果和离线报告

## 失败规则

- Simulator、目标应用或同设备试跑证据缺失：拒绝运行。
- xctrace 超时、非零退出、未生成 `.trace` bundle：任务失败并保留阶段 artifact。
- TOC/Time Profiler 导出缺失或解析失败：拒绝生成数字。
- 无 Running sample：CPU 为 `null`，availability 为 `unavailable_no_running_samples`。
- 不支持的帧、启动、内存、能耗：字段为 `null`，报告展示原因，不得用 `0` 代替。
