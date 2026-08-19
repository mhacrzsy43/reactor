import { mean, percentile, round } from "./stats.mjs";

const UI_THREAD_NAMES = new Set(["UI Thread", "Main Thread", "1.ui"]);
const JS_THREAD_NAMES = new Set([
  "mqt_js",
  "mqt_v_js",
  "com.facebook.react.JavaScript",
  "Lynx_JS",
  "LynxJS",
]);

function numeric(values) {
  return values.filter((value) => Number.isFinite(value));
}

function sumThreadCpu(cpu, names = null) {
  const perName = cpu?.perName ?? {};
  return Object.entries(perName).reduce((sum, [name, value]) => {
    if (!Number.isFinite(value)) return sum;
    if (names && !names.has(name)) return sum;
    return sum + value;
  }, 0);
}

function normalizeIteration(iteration, refreshRate) {
  const measures = Array.isArray(iteration.measures) ? iteration.measures : [];
  const fps = numeric(measures.map((measure) => measure.fps));
  const ram = numeric(measures.map((measure) => measure.ram));
  const cpu = numeric(measures.map((measure) => sumThreadCpu(measure.cpu)));
  const uiCpu = numeric(measures.map((measure) => sumThreadCpu(measure.cpu, UI_THREAD_NAMES)));
  const jsCpu = numeric(measures.map((measure) => sumThreadCpu(measure.cpu, JS_THREAD_NAMES)));
  const lowFpsThreshold = refreshRate * 0.9;

  return {
    status: iteration.status ?? "UNKNOWN",
    durationMs: round(iteration.time ?? 0),
    sampleCount: measures.length,
    fpsMean: round(mean(fps)),
    fpsP10: round(percentile(fps, 0.1)),
    lowFpsSamplePct: round(fps.length ? (fps.filter((value) => value < lowFpsThreshold).length / fps.length) * 100 : null),
    ramMeanMb: round(mean(ram)),
    ramPeakMb: round(ram.length ? Math.max(...ram) : null),
    cpuMeanPct: round(mean(cpu)),
    uiCpuMeanPct: round(mean(uiCpu)),
    jsCpuMeanPct: round(mean(jsCpu)),
  };
}

function aggregate(iterations) {
  const successful = iterations.filter((item) => item.status === "SUCCESS" || item.status === "UNKNOWN");
  const metric = (key) => numeric(successful.map((item) => item[key]));
  return {
    iterationCount: iterations.length,
    successfulIterationCount: successful.length,
    fpsMean: round(mean(metric("fpsMean"))),
    fpsP10: round(mean(metric("fpsP10"))),
    lowFpsSamplePct: round(mean(metric("lowFpsSamplePct"))),
    ramMeanMb: round(mean(metric("ramMeanMb"))),
    ramPeakMb: round(percentile(metric("ramPeakMb"), 0.95)),
    cpuMeanPct: round(mean(metric("cpuMeanPct"))),
    uiCpuMeanPct: round(mean(metric("uiCpuMeanPct"))),
    jsCpuMeanPct: round(mean(metric("jsCpuMeanPct"))),
  };
}

export function normalizeFlashlight(raw, metadata) {
  if (!raw || !Array.isArray(raw.iterations)) {
    throw new Error("Input is not a Flashlight result: missing iterations[]");
  }

  const refreshRate = Number(raw.specs?.refreshRate ?? metadata.refreshRate ?? 60);
  const iterations = raw.iterations.map((iteration) => normalizeIteration(iteration, refreshRate));
  return {
    schemaVersion: 1,
    runId: metadata.runId,
    createdAt: metadata.createdAt ?? new Date().toISOString(),
    framework: metadata.framework,
    platform: metadata.platform,
    scenario: metadata.scenario ?? "unknown",
    adapter: raw.type === "IOS_EXPERIMENTAL" ? "flashlight-ios-experimental" : "flashlight-android",
    buildMode: "release",
    device: {
      refreshRate,
      name: metadata.deviceName ?? null,
      osVersion: metadata.osVersion ?? null,
    },
    source: {
      name: raw.name ?? null,
      status: raw.status ?? null,
      rawFile: metadata.rawFile ?? null,
    },
    iterations,
    summary: aggregate(iterations),
    warnings: raw.type === "IOS_EXPERIMENTAL"
      ? ["Flashlight iOS experimental output currently uses placeholder FPS and RAM values; do not compare those fields."]
      : [],
  };
}
