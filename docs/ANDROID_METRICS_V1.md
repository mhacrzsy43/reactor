# Reactor Android Native Metrics v1

标识：`android-native-v1`

本定义用于 Reactor Benchmark 模式。所有数值必须绑定锁定 Flow、设备信息、原始
Perfetto trace、采集器版本和 Trace Processor 版本。模拟器与物理设备永不混排。

| 字段 | 单位 | 原始来源 | 定义 |
|---|---:|---|---|
| `frameCount` | frame | Perfetto `actual_frame_timeline_slice` | 测量窗口内属于目标应用主 Surface 的有效帧数；排除 Splash Screen、animation leash 和非正时长帧 |
| `frameTimeMeanMs` | ms | Perfetto FrameTimeline | `actual_frame_timeline_slice.dur / 1e6` 的算术平均 |
| `frameTimeP50Ms` | ms | Perfetto FrameTimeline | 上述帧时长的 P50 |
| `frameTimeP95Ms` | ms | Perfetto FrameTimeline | 上述帧时长的 P95 |
| `frameTimeP99Ms` | ms | Perfetto FrameTimeline | 上述帧时长的 P99 |
| `jankFrameCount` | frame | Perfetto FrameTimeline | `jank_type` 不属于 None、Unknown Jank、Prediction Error 的帧数 |
| `jankFramePct` | % | 派生 | `jankFrameCount / frameCount × 100` |
| `overBudgetFramePct` | % | Perfetto FrameTimeline | 帧时长超过 `1000 / refreshRate` ms 的比例 |
| `startupTimeMs` | ms | Android ActivityManager | 冷启动 `am start -W` 返回的 `TotalTime`；与 Flow 正式迭代分开记录 |
| `memoryPssMb` | MiB | `dumpsys meminfo` | 测量完成后目标进程 `TOTAL PSS / 1024` |
| `thermalStatusBefore/After` | level | `dumpsys thermalservice` | Android Thermal Status 数值，0 表示无节流，数值越大热压力越高 |

CPU、采样内存和 FPS 继续保留 Flashlight v0.18 兼容数据，但报告优先展示
Perfetto frame time/jank。原生指标解析器为固定版本 Trace Processor 57.2。
若目标系统不提供 FrameTimeline、trace 为空或解析失败，Reactor 必须保留原始
证据并明确失败，不得用估算值替代。
