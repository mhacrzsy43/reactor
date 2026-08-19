import test from "node:test";
import assert from "node:assert/strict";
import { buildRunPlan } from "../tools/lib/plan.mjs";

const config = { iterationCount: 10, apps: { a: {}, b: {}, c: {} } };
const specification = { scenarios: { list: { durationMs: 100 }, update: { durationMs: 200 } } };

test("run plans are deterministic for a seed and include all tasks", () => {
  const input = { config, specification, platform: "android", frameworks: ["a", "b", "c"], scenarios: ["list", "update"], seed: 42 };
  const first = buildRunPlan(input);
  const second = buildRunPlan(input);
  assert.deepEqual(first.tasks, second.tasks);
  assert.equal(first.hash, second.hash);
  assert.equal(first.tasks.length, 6);
  assert.equal(new Set(first.tasks.map((item) => `${item.framework}:${item.scenario}`)).size, 6);
});

test("different seeds change task order", () => {
  const base = { config, specification, platform: "android", frameworks: ["a", "b", "c"], scenarios: ["list", "update"] };
  assert.notDeepEqual(buildRunPlan({ ...base, seed: 1 }).tasks, buildRunPlan({ ...base, seed: 2 }).tasks);
});
