import { delimiter, dirname } from "node:path";
import { defineAdapter, AdapterRegistry } from "./contracts.mjs";
import { run } from "./process.mjs";

function quoteShell(value) {
  if (process.platform === "win32") return `"${String(value).replaceAll('"', '\\"')}"`;
  return `'${String(value).replaceAll("'", "'\\''")}'`;
}

export function toolEnvironment(tools) {
  return {
    ...process.env,
    JAVA_HOME: tools.javaHome,
    PATH: `${dirname(tools.java)}${delimiter}${dirname(tools.adb)}${delimiter}${process.env.PATH ?? ""}`,
    MAESTRO_CLI_NO_ANALYTICS: "1",
    MAESTRO_CLI_ANALYSIS_NOTIFICATION_DISABLED: "true",
  };
}

export function createBuiltinRegistry(tools) {
  const env = toolEnvironment(tools);
  const registry = new AdapterRegistry();

  registry.register(defineAdapter("automation", {
    id: "maestro",
    platforms: ["android", "ios"],
    commandFor(flow) {
      return `${quoteShell(tools.maestro)} test ${quoteShell(flow)} --no-ansi`;
    },
    async execute({ flow, cwd }) {
      return run(tools.maestro, ["test", flow, "--no-ansi"], { cwd, env });
    },
  }));

  registry.register(defineAdapter("collector", {
    id: "flashlight-android",
    platforms: ["android"],
    async collect({ app, automationCommand, rawFile, durationMs, iterationCount, title, cwd, deviceId }) {
      const collectorEnv = { ...env };
      if (deviceId) collectorEnv.ANDROID_SERIAL = deviceId;
      return run(tools.flashlight, [
        "test",
        "--bundleId", app.androidBundleId,
        "--testCommand", automationCommand,
        "--beforeAllCommand", automationCommand,
        "--duration", String(durationMs),
        "--iterationCount", String(iterationCount),
        "--resultsTitle", title,
        "--resultsFilePath", rawFile,
      ], { cwd, env: collectorEnv });
    },
  }));

  return registry;
}
