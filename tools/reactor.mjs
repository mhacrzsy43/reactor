#!/usr/bin/env node
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { parseArgs, requireOption } from "./lib/args.mjs";
import { loadConfig, getApp } from "./lib/config.mjs";
import { commandExists, run } from "./lib/process.mjs";
import { normalizeFlashlight } from "./lib/normalize.mjs";
import { writeReport } from "./lib/report.mjs";
import { generateFlows } from "./lib/flows.mjs";
import { resolveTools, setupTools } from "./lib/tooling.mjs";
import { createBuiltinRegistry } from "./lib/adapters.mjs";
import { discoverAndroidDevices, discoverDevices } from "./lib/devices.mjs";
import { buildRunPlan } from "./lib/plan.mjs";

const HELP = `Reactor — unified React Native / Flutter / Lynx performance runner

Usage:
  reactor setup
  reactor doctor
  reactor devices
  reactor plan --platform android --all [--seed 42]
  reactor run --platform android --framework react-native --scenario list
  reactor run --platform android --all
  reactor import --framework flutter --platform android --scenario list --input raw.json
  reactor report --input a.json,b.json,c.json [--output results/report.html]

Commands:
  setup    Download pinned private tool copies and generate automation flows
  doctor   Check the shared toolchain and platform collectors
  devices  List Android devices and available iOS simulators
  plan     Create a deterministic, randomized execution plan without running it
  run      Run one app through Maestro + the platform performance adapter
  import   Normalize an existing Flashlight result into Reactor schema
  report   Build one comparison report from normalized result files
`;

function createRunId(framework, platform, scenario) {
  return `${new Date().toISOString().replaceAll(":", "-")}_${framework}_${platform}_${scenario}`;
}

async function doctor(config) {
  const tools = await resolveTools(process.cwd(), config);
  const checks = [
    ["managed Maestro", Boolean(tools.maestro), true],
    ["managed Flashlight", Boolean(tools.flashlight), true],
    ["managed Java", Boolean(tools.java), true],
    ["managed Android device bridge", Boolean(tools.adb), true],
    ["Xcode command line tools", await commandExists("xcrun"), false],
    ["Flutter SDK", await commandExists("flutter"), false],
  ];
  let requiredMissing = false;
  for (const [label, found, required] of checks) {
    requiredMissing ||= required && !found;
    console.log(`${found ? "✓" : required ? "✗" : "○"} ${label}${required ? " (managed)" : ""}`);
  }
  console.log("\nRun `reactor setup` to provision managed dependencies locally.");
  if (requiredMissing) process.exitCode = 1;
}

async function importResult(config, options) {
  const framework = requireOption(options, "framework");
  const platform = requireOption(options, "platform");
  const scenario = requireOption(options, "scenario");
  getApp(config, framework);
  const input = resolve(requireOption(options, "input"));
  const raw = JSON.parse(await readFile(input, "utf8"));
  const runId = options.runId ?? createRunId(framework, platform, scenario);
  const normalized = normalizeFlashlight(raw, {
    framework,
    platform,
    scenario,
    runId,
    rawFile: input,
    refreshRate: config.refreshRate,
    deviceName: options.device,
    osVersion: options.osVersion,
  });
  await mkdir(config.resultsDir, { recursive: true });
  const output = resolve(options.output ?? `${config.resultsDir}/${runId}.json`);
  await writeFile(output, `${JSON.stringify(normalized, null, 2)}\n`, "utf8");
  console.log(output);
  return output;
}

async function runAndroid(config, task, options, registry, experimentDir) {
  const { framework, scenario } = task;
  const app = getApp(config, framework);
  const runId = `${options.experimentId}_${framework}_${scenario}`;
  const rawFile = resolve(experimentDir, "raw", framework, `${scenario}.flashlight.json`);
  const normalizedFile = resolve(experimentDir, "normalized", framework, `${scenario}.json`);
  await mkdir(resolve(rawFile, ".."), { recursive: true });
  await mkdir(resolve(normalizedFile, ".."), { recursive: true });
  const flow = resolve(app.flowDir, "android", `${scenario}.yaml`);
  const automation = registry.get("automation", "maestro", "android");
  const collector = registry.get("collector", "flashlight-android", "android");
  await collector.collect({
    app,
    automationCommand: automation.commandFor(flow),
    rawFile,
    durationMs: Number(options.duration ?? task.durationMs),
    iterationCount: Number(options.iterations ?? task.measuredIterations),
    title: `${app.label} · ${scenario} · Reactor`,
    cwd: process.cwd(),
    deviceId: task.deviceId,
  });
  return importResult(config, { framework, platform: "android", scenario, input: rawFile, output: normalizedFile, runId });
}

async function runBenchmark(config, options) {
  const platform = requireOption(options, "platform");
  const frameworks = options.all ? Object.keys(config.apps) : [requireOption(options, "framework")];
  const scenarioFile = JSON.parse(await readFile(resolve(config.scenarioFile), "utf8"));
  const scenarios = options.all ? Object.keys(scenarioFile.scenarios) : [requireOption(options, "scenario")];
  const tools = await resolveTools(process.cwd(), config);
  if (!tools.maestro || !tools.flashlight || !tools.java || !tools.adb) {
    throw new Error("Managed tools are missing. Run `reactor setup` once; no global installation is required.");
  }
  if (platform === "android") {
    const connected = (await discoverAndroidDevices(tools.adb)).filter((device) => device.state === "device");
    if (!options.device && connected.length === 1) options.device = connected[0].id;
    if (!options.device && connected.length === 0) throw new Error("No Android device is connected. `reactor devices` shows available targets.");
    if (!options.device && connected.length > 1) throw new Error("Multiple Android devices are connected; select one with --device <id>.");
  }
  const plan = buildRunPlan({ config, specification: scenarioFile, platform, frameworks, scenarios, deviceId: options.device ?? null, seed: options.seed ?? Date.now() });
  const experimentDir = resolve(config.resultsDir, "runs", plan.id);
  await mkdir(experimentDir, { recursive: true });
  await writeFile(resolve(experimentDir, "plan.json"), `${JSON.stringify(plan, null, 2)}\n`, "utf8");
  const registry = createBuiltinRegistry(tools);
  const outputs = [];
  for (const task of plan.tasks) {
    const taskOptions = { ...options, experimentId: plan.id };
    if (platform === "android") outputs.push(await runAndroid(config, task, taskOptions, registry, experimentDir));
    else throw new Error("The iOS adapter will be enabled after its xctrace frame/energy parser is verified; Flashlight iOS placeholder FPS/RAM is intentionally rejected.");
  }
  if (outputs.length > 1) {
    const report = await writeReport(outputs, resolve(experimentDir, "report.html"));
    console.log(`Comparison report: ${report.outputFile}`);
  }
  return outputs;
}

async function main() {
  const { command, options } = parseArgs(process.argv.slice(2));
  if (command === "help" || options.help) {
    console.log(HELP);
    return;
  }
  const config = await loadConfig(process.cwd(), options.config);
  if (command === "setup") {
    await setupTools(process.cwd(), config, { onStatus: (message) => console.log(`→ ${message}`) });
    const flows = await generateFlows(process.cwd(), config);
    console.log(`Managed tools ready; generated ${flows.written.length} automation flows.`);
    return;
  }
  if (command === "doctor") {
    await doctor(config);
    return;
  }
  if (command === "devices") {
    const tools = await resolveTools(process.cwd(), config);
    if (!tools.adb) throw new Error("Managed Android tools are missing. Run `reactor setup`.");
    const devices = await discoverDevices(tools);
    console.log(JSON.stringify(devices, null, 2));
    return;
  }
  if (command === "plan") {
    const platform = requireOption(options, "platform");
    const { specification } = await generateFlows(process.cwd(), config);
    const frameworks = options.all ? Object.keys(config.apps) : [requireOption(options, "framework")];
    const scenarios = options.all ? Object.keys(specification.scenarios) : [requireOption(options, "scenario")];
    const plan = buildRunPlan({ config, specification, platform, frameworks, scenarios, deviceId: options.device ?? null, seed: options.seed ?? Date.now() });
    if (options.output) {
      await mkdir(resolve(options.output, ".."), { recursive: true });
      await writeFile(resolve(options.output), `${JSON.stringify(plan, null, 2)}\n`, "utf8");
      console.log(resolve(options.output));
    } else console.log(JSON.stringify(plan, null, 2));
    return;
  }
  if (command === "generate") {
    const flows = await generateFlows(process.cwd(), config);
    console.log(`Generated ${flows.written.length} automation flows.`);
    return;
  }
  if (command === "run") {
    const outputs = await runBenchmark(config, options);
    for (const output of outputs) console.log(`Normalized result: ${output}`);
    return;
  }
  if (command === "import") {
    await importResult(config, options);
    return;
  }
  if (command === "report") {
    const input = requireOption(options, "input").split(",").filter(Boolean);
    const output = options.output ?? `${config.resultsDir}/report.html`;
    const result = await writeReport(input, output);
    console.log(result.outputFile);
    return;
  }
  throw new Error(`Unknown command "${command}"\n\n${HELP}`);
}

main().catch((error) => {
  console.error(`Reactor: ${error.message}`);
  process.exitCode = 1;
});
