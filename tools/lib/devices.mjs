import { commandExists, run } from "./process.mjs";

export function parseAdbDevices(output) {
  return output.split(/\r?\n/).slice(1).map((line) => line.trim()).filter(Boolean).map((line) => {
    const [id, state, ...properties] = line.split(/\s+/);
    const metadata = Object.fromEntries(properties.filter((item) => item.includes(":")) .map((item) => item.split(":", 2)));
    return { id, state, platform: "android", name: metadata.model ?? metadata.device ?? null, metadata };
  });
}

export async function discoverAndroidDevices(adb) {
  const { stdout } = await run(adb, ["devices", "-l"], { capture: true });
  return parseAdbDevices(stdout);
}

export async function discoverIosDevices() {
  if (process.platform !== "darwin" || !await commandExists("xcrun")) return [];
  const { stdout } = await run("xcrun", ["simctl", "list", "devices", "available", "--json"], { capture: true });
  const parsed = JSON.parse(stdout);
  return Object.entries(parsed.devices ?? {}).flatMap(([runtime, devices]) => devices.map((device) => ({
    id: device.udid,
    state: device.state?.toLowerCase(),
    platform: "ios",
    name: device.name,
    metadata: { runtime, isAvailable: device.isAvailable },
  })));
}

export async function discoverDevices(tools) {
  const [android, ios] = await Promise.all([discoverAndroidDevices(tools.adb), discoverIosDevices()]);
  return [...android, ...ios];
}
