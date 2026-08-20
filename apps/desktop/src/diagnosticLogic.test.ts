import test from "node:test";
import assert from "node:assert/strict";
import { conservativeAndroidDiagnosticPlan, diagnosticContextKey, diagnosticRunIdentity, diagnosticWorkbenchKey, groupDiagnosticRunsByFlow, historicalRerunBlockingReferences, isUsableDiagnosticResult, preferredDiagnosticFlowHash, preferredDiagnosticRun, RequestTokens, sourceMapStatus, telemetrySlopePerMinute } from "./diagnosticLogic.ts";
import type { DiagnosticRunSummary, NormalizedResult } from "./types.ts";

function result(synthetic: boolean, successfulIterationCount: number): NormalizedResult {
  return {
    runId: "run-1",
    framework: "react-native",
    platform: "android",
    scenario: "list",
    adapter: "test",
    flowHash: "flow-a",
    source: { synthetic },
    summary: { iterationCount: 1, successfulIterationCount },
    warnings: [],
  };
}

test("Android Diagnose plan uses supported collector and bounded resources", () => {
  const plan = conservativeAndroidDiagnosticPlan(18_000, 10);
  assert.deepEqual(plan.collectors, [{ collector: "hermes-cpu", required: false }]);
  assert.equal(plan.schemaVersion, 1);
  assert.equal(plan.mode, "in_band");
  assert.equal(plan.resourceLimits.maxDurationMs, 54_000);
  assert.ok(plan.resourceLimits.maxArtifactBytes <= 1024 * 1024 * 1024);
  assert.ok(plan.resourceLimits.maxEvents <= 5_000_000);
  assert.ok(plan.resourceLimits.maxSamples <= 20_000_000);
});

test("live telemetry slope reports memory growth per minute", () => {
  assert.equal(telemetrySlopePerMinute([
    { timeMs: 1_000, value: 100 },
    { timeMs: 31_000, value: 105 },
    { timeMs: 61_000, value: 110 },
  ]), 10);
  assert.equal(telemetrySlopePerMinute([{ timeMs: 1_000, value: 100 }]), undefined);
});

test("historical rerun blocks prompt, secret, and TOTP refs including nested repeats", () => {
  const flow = {
    schemaVersion: 1,
    id: "inputs",
    name: "Inputs",
    appId: "com.example.app",
    platform: "android" as const,
    setup: [{ action: "input_text" as const, target: {}, value: { promptRef: "user" }, clearBefore: true }],
    measured: [{ action: "repeat" as const, times: 1, steps: [
      { action: "input_text" as const, target: {}, value: { secretRef: "password" }, clearBefore: true },
      { action: "input_text" as const, target: {}, value: { totpRef: "otp" }, clearBefore: true },
    ] }],
    teardown: [],
  };
  assert.deepEqual(historicalRerunBlockingReferences(flow), ["promptRef", "secretRef", "totpRef"]);
  assert.deepEqual(historicalRerunBlockingReferences({ ...flow, setup: [], measured: [] }), []);
});

test("usable diagnostic Run rejects synthetic and zero-success results", () => {
  assert.equal(isUsableDiagnosticResult(result(false, 1)), true);
  assert.equal(isUsableDiagnosticResult(result(true, 1)), false);
  assert.equal(isUsableDiagnosticResult(result(false, 0)), false);
});

test("context key changes across Flow and framework boundaries", () => {
  assert.equal(diagnosticContextKey("flow-a", "react-native"), "no-job:no-run:flow-a:react-native");
  assert.notEqual(diagnosticContextKey("flow-a", "react-native"), diagnosticContextKey("flow-b", "react-native"));
  assert.notEqual(diagnosticContextKey("flow-a", "react-native"), diagnosticContextKey("flow-a", "flutter"));
});

test("workbench key isolates exact job, run, Flow, and framework", () => {
  const key = diagnosticWorkbenchKey("job-a", "run-a", "flow-a", "react-native");
  assert.equal(key, "job-a:run-a:flow-a:react-native");
  assert.notEqual(key, diagnosticWorkbenchKey("job-b", "run-a", "flow-a", "react-native"));
  assert.notEqual(key, diagnosticWorkbenchKey("job-a", "run-b", "flow-a", "react-native"));
  assert.notEqual(key, diagnosticWorkbenchKey("job-a", "run-a", "flow-b", "react-native"));
  assert.notEqual(key, diagnosticWorkbenchKey("job-a", "run-a", "flow-a", "flutter"));
});

function diagnosticRun(overrides: Partial<DiagnosticRunSummary> = {}): DiagnosticRunSummary {
  const normalized = result(false, 1);
  return {
    jobId: "job-1",
    runId: normalized.runId,
    flowHash: normalized.flowHash,
    framework: normalized.framework,
    platform: normalized.platform,
    createdAt: "2026-08-20T12:00:00Z",
    successfulIterationCount: 1,
    iterationCount: 1,
    synthetic: false,
    lockAvailable: true,
    result: normalized,
    ...overrides,
  };
}

test("historical Runs group by flowHash and sort newest first", () => {
  const groups = groupDiagnosticRunsByFlow([
    diagnosticRun({ runId: "older", createdAt: "2026-08-19T12:00:00Z" }),
    diagnosticRun({ runId: "other", flowHash: "flow-b", createdAt: "2026-08-18T12:00:00Z" }),
    diagnosticRun({ runId: "newer", createdAt: "2026-08-20T12:00:00Z" }),
  ]);
  assert.equal(groups.length, 2);
  assert.deepEqual(groups[0]?.runs.map((run) => run.runId), ["newer", "older"]);
});

test("current Flow Studio flow is preferred, with explicit Run selection retained", () => {
  const runs = [diagnosticRun({ runId: "a" }), diagnosticRun({ runId: "b", flowHash: "flow-b" })];
  assert.equal(preferredDiagnosticFlowHash(runs, "flow-b"), "flow-b");
  assert.equal(preferredDiagnosticFlowHash(runs, "missing"), "flow-a");
  assert.equal(preferredDiagnosticRun(runs, "flow-a", diagnosticRunIdentity(runs[0]))?.runId, "a");
  assert.equal(preferredDiagnosticRun(runs, "flow-b", diagnosticRunIdentity(runs[0]))?.runId, "b");
});

test("Run selection identity includes jobId when runId values collide", () => {
  const runs = [diagnosticRun({ jobId: "job-a", runId: "same" }), diagnosticRun({ jobId: "job-b", runId: "same" })];
  assert.equal(diagnosticRunIdentity(runs[1]), "job-b:same");
  assert.equal(preferredDiagnosticRun(runs, "flow-a", "job-b:same")?.jobId, "job-b");
});

test("request tokens reject stale async completions", () => {
  const requests = new RequestTokens(["profile"] as const);
  const stale = requests.start("profile");
  const current = requests.start("profile");
  assert.equal(requests.isCurrent("profile", stale), false);
  assert.equal(requests.isCurrent("profile", current), true);
  requests.cancel("profile");
  assert.equal(requests.isCurrent("profile", current), false);
});

test("Source Map status distinguishes absent, zero, and successful mapping", () => {
  assert.equal(sourceMapStatus({ state: "not-collected", mappedCount: 0 }), "尚未导入 Source Map");
  assert.equal(sourceMapStatus({ state: "available", mappedCount: 0 }), "Source Map 已加载，0 个位置可映射");
  assert.equal(sourceMapStatus({ state: "available", mappedCount: 3 }), "3 个位置已映射");
});
