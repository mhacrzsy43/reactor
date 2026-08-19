import { createHash } from "node:crypto";

function seededRandom(seed) {
  let value = seed >>> 0;
  return () => {
    value += 0x6d2b79f5;
    let result = value;
    result = Math.imul(result ^ (result >>> 15), result | 1);
    result ^= result + Math.imul(result ^ (result >>> 7), result | 61);
    return ((result ^ (result >>> 14)) >>> 0) / 4294967296;
  };
}

function shuffle(items, seed) {
  const result = [...items];
  const random = seededRandom(seed);
  for (let index = result.length - 1; index > 0; index -= 1) {
    const swap = Math.floor(random() * (index + 1));
    [result[index], result[swap]] = [result[swap], result[index]];
  }
  return result;
}

export function buildRunPlan({ config, specification, platform, frameworks, scenarios, deviceId = null, seed = Date.now() }) {
  const tasks = frameworks.flatMap((framework) => scenarios.map((scenario) => {
    const scenarioSpec = specification.scenarios[scenario];
    if (!scenarioSpec) throw new Error(`Unknown scenario: ${scenario}`);
    if (!config.apps[framework]) throw new Error(`Unknown framework: ${framework}`);
    return {
      framework,
      scenario,
      platform,
      deviceId,
      durationMs: scenarioSpec.durationMs,
      warmupIterations: 1,
      measuredIterations: config.iterationCount,
    };
  }));
  const numericSeed = Number(seed) >>> 0;
  const ordered = shuffle(tasks, numericSeed).map((task, index) => ({ ...task, order: index + 1 }));
  const planCore = { schemaVersion: 1, platform, deviceId, seed: numericSeed, tasks: ordered };
  const hash = createHash("sha256").update(JSON.stringify(planCore)).digest("hex");
  return { ...planCore, id: `${new Date().toISOString().replaceAll(":", "-")}_${hash.slice(0, 10)}`, hash };
}
