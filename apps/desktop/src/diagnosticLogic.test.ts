import test from "node:test";
import assert from "node:assert/strict";
import { diagnosticContextKey, isUsableDiagnosticResult, RequestTokens, sourceMapStatus } from "./diagnosticLogic.ts";
import type { NormalizedResult } from "./types.ts";

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

test("usable diagnostic Run rejects synthetic and zero-success results", () => {
  assert.equal(isUsableDiagnosticResult(result(false, 1)), true);
  assert.equal(isUsableDiagnosticResult(result(true, 1)), false);
  assert.equal(isUsableDiagnosticResult(result(false, 0)), false);
});

test("context key changes across Flow and framework boundaries", () => {
  assert.equal(diagnosticContextKey("flow-a", "react-native"), "flow-a:react-native");
  assert.notEqual(diagnosticContextKey("flow-a", "react-native"), diagnosticContextKey("flow-b", "react-native"));
  assert.notEqual(diagnosticContextKey("flow-a", "react-native"), diagnosticContextKey("flow-a", "flutter"));
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
