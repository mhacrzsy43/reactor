import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

export async function loadConfig(cwd, configPath = "reactor.config.json") {
  const absolutePath = resolve(cwd, configPath);
  const config = JSON.parse(await readFile(absolutePath, "utf8"));

  if (config.schemaVersion !== 1 || typeof config.apps !== "object") {
    throw new Error(`Unsupported or invalid config: ${absolutePath}`);
  }

  return {
    ...config,
    absolutePath,
    resultsDir: resolve(cwd, config.resultsDir ?? "results"),
  };
}

export function getApp(config, framework) {
  const app = config.apps[framework];
  if (!app) {
    throw new Error(`Unknown framework "${framework}". Expected one of: ${Object.keys(config.apps).join(", ")}`);
  }
  return app;
}
