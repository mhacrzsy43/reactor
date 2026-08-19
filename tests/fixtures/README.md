# Reactor 回放样本

`perfetto-react-native-list.pftrace` 是在 Android Emulator 上执行锁定的 React Native
列表 Flow 后保存的固定 Perfetto FrameTimeline。它只用于解析器回归，不作为框架性能结论。

- SHA-256：`854c53865724742aabcec31e0db55c4434b84c9767d9ad132d469d3d3784b769`
- 应用：`com.reactor.bench.reactnative`
- Trace Processor：`57.2`
- 预期：498 帧、P95 20.140619 ms、P99 45.129122 ms、98 个 jank 帧

`perfetto-frame-metrics.csv` 是同一版本协议的轻量解析 fixture，供默认单元测试使用。
真实 trace 回放测试需要先完成 Reactor 受管工具安装，然后执行：

```sh
cargo test -p reactor-runner replays_fixed_perfetto_trace_and_rejects_corrupt_trace -- --ignored
```

`xctrace-time-profiler-toc.xml` 与 `xctrace-time-profile.xml` 是脱敏、可读的小型 iOS
Simulator xctrace 导出 fixture，覆盖录制时长、工具版本、Simulator 元数据、weight 定义与
引用解析。它们只验证 `ios-native-v1` 解析协议；帧、启动、内存和能耗在 Simulator 不可用时
必须保留显式 availability 状态，不允许推导占位数字。

`react-profiler-baseline.json` 与 `react-profiler-regressed.json` 覆盖 React DevTools
Profiler 导入、组件 Render/Commit 统计、重复渲染规则和 Profile Diff。
`hermes-cpu-profile.json` 覆盖 Hermes/Chrome CPU 热点；`hermes-bundle-profile.json`
配合 `hermes-bundle.js.map` 验证从 bundle 位置映射回 TypeScript 源码。
