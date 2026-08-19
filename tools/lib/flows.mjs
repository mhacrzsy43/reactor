import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";

const quote = (value) => JSON.stringify(String(value));

function header(appId) {
  return `appId: ${appId}\n---\n`;
}

function launch(readyText) {
  return `- launchApp:\n    clearState: true\n- assertVisible: ${quote(readyText)}\n`;
}

function flowFor(appId, name, scenario) {
  let body = header(appId);
  if (name === "startup") return body + launch(scenario.readyText);

  body += launch("Reactor ready");
  body += `- tapOn: ${quote(scenario.entryText)}\n- assertVisible: ${quote(scenario.readyText)}\n`;
  if (name === "list") {
    body += `- repeat:\n    times: ${scenario.upSwipes}\n    commands:\n      - swipe:\n          direction: UP\n          duration: ${scenario.swipeDurationMs}\n`;
    body += `- repeat:\n    times: ${scenario.downSwipes}\n    commands:\n      - swipe:\n          direction: DOWN\n          duration: ${scenario.swipeDurationMs}\n`;
  } else {
    body += `- extendedWaitUntil:\n    visible: ${quote(scenario.completeText)}\n    timeout: ${scenario.workloadMs + 4000}\n`;
  }
  return body;
}

export async function generateFlows(cwd, config) {
  const scenarioPath = resolve(cwd, config.scenarioFile);
  const specification = JSON.parse(await readFile(scenarioPath, "utf8"));
  const written = [];
  for (const [framework, app] of Object.entries(config.apps)) {
    for (const platform of ["android", "ios"]) {
      const appId = platform === "android" ? app.androidBundleId : app.iosBundleId;
      for (const [name, scenario] of Object.entries(specification.scenarios)) {
        const output = resolve(cwd, app.flowDir, platform, `${name}.yaml`);
        await mkdir(dirname(output), { recursive: true });
        await writeFile(output, flowFor(appId, name, scenario), "utf8");
        written.push({ framework, platform, scenario: name, output });
      }
    }
  }
  return { written, specification };
}
