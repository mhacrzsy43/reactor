# Reactor CI 性能回归门禁

## 构建 CLI

```sh
cargo build --release -p reactor-cli
```

## 比较两次黑盒性能结果

```sh
./target/release/reactor ci \
  --baseline results/runs/8bad7514-ed8b-4747-af5d-2a884c6e0934/result.json \
  --current results/runs/f01c9166-3707-411d-95e0-76542a702fb2/result.json \
  --output-dir target/reactor-ci
```

输入既可以是一个 `NormalizedResult` JSON，也可以是同格式数组。Reactor 会先验证平台、
设备类别、Flow、场景和指标定义是否兼容，再应用版本化回归阈值。

## 同时检查 RN 组件 Profile

```sh
./target/release/reactor ci \
  --baseline results/runs/8bad7514-ed8b-4747-af5d-2a884c6e0934/result.json \
  --current results/runs/f01c9166-3707-411d-95e0-76542a702fb2/result.json \
  --baseline-profile tests/fixtures/react-profiler-baseline.json \
  --current-profile tests/fixtures/react-profiler-regressed.json \
  --output-dir target/reactor-ci
```

如 Profile 中保存的是 bundle 位置，可增加 `--source-map path/to/index.bundle.map`。
Source Map 在本机处理，不会上传。

## 输出与退出码

| 文件或退出码 | 含义 |
|---|---|
| `analysis.json` | 机器可读的兼容性、指标 diff、规则 verdict 和 Profile Diff |
| `junit.xml` | CI 平台可直接发布的测试结果；回归为 failure，不兼容为 error |
| `report.html` | 可独立打开的静态回归报告 |
| `0` | 通过，未发现超过阈值的回归 |
| `2` | 检测到黑盒指标或组件 Profile 回归 |
| `3` | 基线不兼容，拒绝产生误导性比较 |

CI 判定完全由本地确定性规则生成，不调用 AI。AI 解读只能在判定完成后作为附加说明。

## 通用 CI 脚本

```sh
set +e
./target/release/reactor ci \
  --baseline "$REACTOR_BASELINE" \
  --current "$REACTOR_CURRENT" \
  --output-dir reactor-ci
status=$?
set -e

# 无论通过或失败，都先上传 reactor-ci/ 作为构建产物，再透传 Reactor 退出码。
exit "$status"
```

不要把退出码 `3` 当成普通回归；它表示实验条件不同，应重新选择兼容基线。
