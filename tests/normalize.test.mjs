import test from "node:test";
import assert from "node:assert/strict";
import { normalizeFlashlight } from "../tools/lib/normalize.mjs";

test("normalizes Flashlight measures and aggregates iterations", () => {
  const raw = {
    name: "fixture",
    status: "SUCCESS",
    specs: { refreshRate: 60 },
    iterations: [{
      status: "SUCCESS",
      time: 1000,
      measures: [
        { fps: 60, ram: 100, time: 0, cpu: { perName: { "UI Thread": 20, mqt_js: 10 }, perCore: {} } },
        { fps: 48, ram: 120, time: 500, cpu: { perName: { "UI Thread": 40, mqt_js: 20 }, perCore: {} } }
      ]
    }]
  };
  const result = normalizeFlashlight(raw, { framework: "react-native", platform: "android", scenario: "list", runId: "test" });
  assert.equal(result.summary.fpsMean, 54);
  assert.equal(result.summary.lowFpsSamplePct, 50);
  assert.equal(result.summary.ramMeanMb, 110);
  assert.equal(result.summary.ramPeakMb, 120);
  assert.equal(result.summary.cpuMeanPct, 45);
  assert.equal(result.summary.uiCpuMeanPct, 30);
  assert.equal(result.summary.jsCpuMeanPct, 15);
  assert.equal(result.scenario, "list");
});

test("flags experimental iOS placeholder metrics", () => {
  const result = normalizeFlashlight({ type: "IOS_EXPERIMENTAL", iterations: [] }, {
    framework: "flutter", platform: "ios", runId: "test"
  });
  assert.equal(result.adapter, "flashlight-ios-experimental");
  assert.equal(result.warnings.length, 1);
});
