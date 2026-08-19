# Reactor benchmark protocol

Reactor compares React Native, Flutter, and Lynx as three independent release applications. The host-side Reactor process owns automation, native metric collection, normalization, and reporting.

## Non-negotiable controls

- Run all frameworks on the same physical device, OS build, refresh rate, brightness, and power mode.
- Use release builds only. Debug, hot reload, inspectors, and framework overlays must be disabled.
- Keep the device unplugged or plugged in consistently and record thermal state before each group.
- Run one unmeasured warm-up iteration, followed by ten measured iterations by default.
- Randomize framework order per complete run to reduce temperature and cache-order bias.
- Reset app data before cold-start runs. Do not reset data between warm interaction runs.
- Use bundled local assets and deterministic generated data. Network access is forbidden during measured windows.
- Report raw iterations and dispersion. Never publish only a single score.

## Implementation equivalence

The demos implement the same visible outcome and workload, using each framework's documented production best practice:

- React Native: virtualized list and native/UI-thread animation driver.
- Flutter: lazy slivers and `AnimationController`/transform animation.
- Lynx: list container and CSS/main-thread animation facilities.

This measures realistic optimized applications, not identical source-level algorithms. A deliberately naive implementation can be added later as a separate experiment, but must not be mixed into the primary report.

## Scenarios

### `startup`

Cold-launch the process after clearing app state. Stop when the semantic marker `Reactor ready` is visible. Primary metrics are process launch-to-first-frame and launch-to-ready; runtime-local timers are secondary diagnostics only.

### `list`

Render 1,000 deterministic cards of fixed height, then perform eight 800 ms upward swipes and two downward swipes. No images or network access. Primary metrics are frame pacing, low-FPS samples, CPU, and peak memory.

### `update`

Render 500 deterministic rows. Every 100 ms, update a deterministic 10% subset for 8 seconds. The app exposes `Update complete` when finished. Primary metrics are frame pacing, CPU, and completion wall time.

### `animation`

Animate 64 colored tiles for 8 seconds using transform and opacity only. The app exposes `Animation complete` when finished. Primary metrics are frame pacing, UI-thread CPU, and energy when the platform collector supports it.

## Metric rules

- FPS and frame pacing must come from the operating-system/rendering pipeline collector. JavaScript/Dart requestAnimationFrame values are diagnostic and cannot replace native frame metrics.
- CPU is reported as sampled process/thread CPU and can exceed 100% on multicore devices.
- Memory is resident/proportional memory as exposed by the selected platform adapter. Do not compare different memory definitions in one column.
- Android and iOS results are separate experiments and must never be merged into a single framework ranking.
- Experimental or placeholder collector fields are rejected or visibly marked as non-comparable.

## Required report metadata

Every normalized result records framework, platform, scenario, app version, build mode, device, OS version, refresh rate, collector versions, iteration count, raw artifact location, and warnings.
