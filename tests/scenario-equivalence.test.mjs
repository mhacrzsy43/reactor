import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const frameworks = ["react-native", "flutter", "lynx"];
const platforms = ["android", "ios"];
const scenarioNames = ["startup", "list", "update", "animation"];

async function read(relativePath) {
  return readFile(path.join(root, relativePath), "utf8");
}

function withoutAppId(yaml) {
  return yaml.replace(/^appId:.*\n/, "");
}

test("all framework sources implement the versioned workload constants", async () => {
  const specification = JSON.parse(await read("scenarios/scenarios.json"));
  const sources = {
    "react-native": await read("demos/react-native/App.tsx"),
    flutter: await read("demos/flutter/lib/main.dart"),
    lynx: await read("demos/lynx/src/pages/main/App.tsx"),
  };

  assert.equal(specification.schemaVersion, 1);
  assert.equal(specification.dataSeed, 20260818);
  assert.equal(specification.scenarios.list.itemCount, 1000);
  assert.equal(specification.scenarios.list.itemExtent, 96);
  assert.equal(specification.scenarios.update.itemCount, 500);
  assert.equal(specification.scenarios.update.updateRatio, 0.1);
  assert.equal(specification.scenarios.update.intervalMs, 100);
  assert.equal(specification.scenarios.update.workloadMs, 8000);
  assert.equal(specification.scenarios.animation.tileCount, 64);
  assert.equal(specification.scenarios.animation.workloadMs, 8000);

  const patterns = {
    "react-native": [
      /const DATA_SEED = 20260818;/,
      /const LIST_COUNT = 1000;/,
      /const UPDATE_COUNT = 500;/,
      /const UPDATE_TICKS = 80;/,
      /const UPDATE_BATCH = 50;/,
      /const TILE_COUNT = 64;/,
      />>> 0\) % 10000/,
    ],
    flutter: [
      /const dataSeed = 20260818;/,
      /const listCount = 1000;/,
      /const updateCount = 500;/,
      /const updateTicks = 80;/,
      /const updateBatch = 50;/,
      /const tileCount = 64;/,
      /& 0xffffffff\)/,
    ],
    lynx: [
      /const DATA_SEED = 20260818/,
      /const LIST_COUNT = 1000/,
      /const UPDATE_COUNT = 500/,
      /const UPDATE_TICKS = 80/,
      /const UPDATE_BATCH = 50/,
      /const TILE_COUNT = 64/,
      />>> 0\) % 10000/,
    ],
  };

  for (const framework of frameworks) {
    for (const pattern of patterns[framework]) {
      assert.match(sources[framework], pattern, `${framework} missing ${pattern}`);
    }
    for (const marker of [
      "Reactor ready",
      "List scenario",
      "List ready",
      "Update scenario",
      "Update ready",
      "Update complete",
      "Animation scenario",
      "Animation ready",
      "Animation complete",
    ]) {
      assert.ok(sources[framework].includes(marker), `${framework} missing marker ${marker}`);
    }
  }

  for (const marker of [
    "Reactor ready",
    "List scenario",
    "List ready",
    "Update scenario",
    "Update ready",
    "Animation scenario",
    "Animation ready",
  ]) {
    assert.ok(
      sources.lynx.includes(`accessibility-label=${marker === "Reactor ready" ? `"${marker}"` : marker.includes("scenario") ? "{text}" : "{title}"}`),
      `lynx missing accessibility label for ${marker}`,
    );
  }
  assert.match(sources.lynx, /accessibility-label=\{status\}/);
  assert.match(await read("demos/lynx/app.config.ts"), /enableAccessibilityElement:\s*true/);
  const lynxBridge = await read(
    "demos/lynx/android/app/src/main/java/com/reactor/bench/lynx/LynxAutomationAccessibilityBridge.kt",
  );
  for (const selector of [
    "reactor-ready",
    "list-scenario",
    "update-scenario",
    "animation-scenario",
    "list-ready",
    "update-ready",
    "update-complete",
    "animation-ready",
    "animation-complete",
  ]) {
    assert.ok(sources.lynx.includes(selector), `lynx missing semantic selector ${selector}`);
    assert.ok(lynxBridge.includes(selector), `lynx bridge missing semantic selector ${selector}`);
  }
});

test("all generated Maestro flows are identical except for the package id", async () => {
  const config = JSON.parse(await read("reactor.config.json"));
  for (const platform of platforms) {
    for (const scenario of scenarioNames) {
      const flows = await Promise.all(
        frameworks.map(framework =>
          read(`automation/generated/${framework}/${platform}/${scenario}.yaml`),
        ),
      );
      assert.equal(withoutAppId(flows[0]), withoutAppId(flows[1]));
      assert.equal(withoutAppId(flows[1]), withoutAppId(flows[2]));
      frameworks.forEach((framework, index) => {
        const expected = config.apps[framework][`${platform}BundleId`];
        assert.match(flows[index], new RegExp(`^appId: ${expected}$`, "m"));
      });
    }
  }
});

test("the shared 32-bit data function produces stable samples", () => {
  const value = (index, tick = 0) =>
    (20260818 + Math.imul(index, 1103515245) + Math.imul(tick, 2654435761)) >>> 0;
  assert.equal(value(0) % 10000, 818);
  assert.equal(value(1) % 10000, 6063);
  assert.equal(value(499, 80) % 10000, 7561);
});
