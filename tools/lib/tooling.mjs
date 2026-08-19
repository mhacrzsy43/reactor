import { createHash } from "node:crypto";
import { createWriteStream } from "node:fs";
import { access, chmod, mkdir, readFile, readdir, rename, rm, stat, writeFile } from "node:fs/promises";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { basename, join, resolve, sep } from "node:path";
import { run } from "./process.mjs";

async function exists(path) {
  try { await access(path); return true; } catch { return false; }
}

async function digest(path, algorithm = "sha256") {
  const hash = createHash(algorithm);
  const stream = (await import("node:fs")).createReadStream(path);
  for await (const chunk of stream) hash.update(chunk);
  return hash.digest("hex");
}

async function download(url, destination) {
  const response = await fetch(url, { redirect: "follow" });
  if (!response.ok || !response.body) throw new Error(`Download failed (${response.status}): ${url}`);
  await mkdir(resolve(destination, ".."), { recursive: true });
  await pipeline(Readable.fromWeb(response.body), createWriteStream(destination));
}

async function verify(path, expected, algorithm = "sha256") {
  const actual = await digest(path, algorithm);
  if (actual.toLowerCase() !== expected.toLowerCase()) {
    throw new Error(`Checksum mismatch for ${basename(path)}: expected ${expected}, received ${actual}`);
  }
}

async function installArchive({ archive, target, type }) {
  const staging = `${target}.staging`;
  await rm(staging, { recursive: true, force: true });
  await mkdir(staging, { recursive: true });
  if (type === "zip" && process.platform === "win32") {
    await run("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", "Expand-Archive", "-LiteralPath", archive, "-DestinationPath", staging, "-Force"]);
  } else if (type === "zip") await run("unzip", ["-q", archive, "-d", staging]);
  else await run("tar", ["-xzf", archive, "-C", staging]);
  await rm(target, { recursive: true, force: true });
  await rename(staging, target);
}

async function findFile(root, predicate, depth = 5) {
  if (depth < 0) return null;
  if (!await exists(root)) return null;
  for (const entry of await readdir(root, { withFileTypes: true })) {
    const path = join(root, entry.name);
    if (entry.isFile() && predicate(path)) return path;
    if (entry.isDirectory()) {
      const nested = await findFile(path, predicate, depth - 1);
      if (nested) return nested;
    }
  }
  return null;
}

function platformNames() {
  const os = process.platform === "darwin" ? "mac" : process.platform === "linux" ? "linux" : process.platform === "win32" ? "windows" : null;
  const arch = process.arch === "arm64" ? "aarch64" : process.arch === "x64" ? "x64" : null;
  if (!os || !arch) throw new Error(`Unsupported host ${process.platform}/${process.arch}`);
  return { os, arch };
}

async function adoptiumAsset(version) {
  const { os, arch } = platformNames();
  const api = `https://api.adoptium.net/v3/assets/latest/${version}/hotspot?architecture=${arch}&image_type=jre&os=${os}&vendor=eclipse`;
  const response = await fetch(api);
  if (!response.ok) throw new Error(`Unable to resolve managed Java runtime (${response.status})`);
  const assets = await response.json();
  const pkg = assets?.[0]?.binary?.package;
  if (!pkg?.link || !pkg?.checksum) throw new Error("Adoptium response did not contain a downloadable JRE");
  return { url: pkg.link, checksum: pkg.checksum };
}

export function toolLayout(cwd, config) {
  const root = resolve(cwd, ".reactor/tools");
  const maestroRoot = join(root, `maestro-${config.maestroVersion}`);
  const flashlightRoot = join(root, `flashlight-${config.flashlightVersion}`);
  const javaRoot = join(root, `jre-${config.jreVersion}`);
  const androidToolsRoot = join(root, `android-platform-tools-${config.androidPlatformTools.version}`);
  return {
    root,
    maestroRoot,
    flashlightRoot,
    javaRoot,
    androidToolsRoot,
    downloads: resolve(cwd, ".reactor/downloads"),
  };
}

export async function resolveTools(cwd, config) {
  const layout = toolLayout(cwd, config);
  const override = config.maestroOverridePath ? resolve(cwd, config.maestroOverridePath) : null;
  if (override && !await exists(override)) {
    throw new Error(`maestroOverridePath does not exist: ${override}`);
  }
  const maestro = override ?? await findFile(layout.maestroRoot, (path) => ["maestro", "maestro.bat"].includes(basename(path).toLowerCase()));
  const flashlight = await findFile(layout.flashlightRoot, (path) => /^flashlight(?:-macos|-linux|-win\.exe)?$/.test(basename(path).toLowerCase()));
  const java = await findFile(layout.javaRoot, (path) => ["java", "java.exe"].includes(basename(path).toLowerCase()) && path.split(sep).at(-2)?.toLowerCase() === "bin");
  const adb = await findFile(layout.androidToolsRoot, (path) => ["adb", "adb.exe"].includes(basename(path).toLowerCase()));
  return {
    ...layout,
    maestro,
    flashlight,
    java,
    adb,
    javaHome: java ? resolve(java, "../..") : null,
    maestroSource: override ? "override" : "managed-release",
  };
}

async function installMaestro(cwd, config, layout) {
  const zip = join(layout.downloads, `maestro-${config.maestroVersion}.zip`);
  const checksumFile = join(layout.downloads, `maestro-${config.maestroVersion}.sha256`);
  const base = `https://github.com/mobile-dev-inc/Maestro/releases/download/cli-${config.maestroVersion}`;
  if (!await exists(zip)) await download(`${base}/maestro.zip`, zip);
  if (!await exists(checksumFile)) await download(`${base}/checksums_sha256.txt`, checksumFile);
  const checksumText = await readFile(checksumFile, "utf8");
  const checksum = checksumText.split("\n").find((line) => line.includes("maestro.zip"))?.trim().split(/\s+/)[0];
  if (!checksum) throw new Error("Maestro checksum file did not list maestro.zip");
  await verify(zip, checksum);
  await installArchive({ archive: zip, target: layout.maestroRoot, type: "zip" });
}

async function installFlashlight(config, layout) {
  const host = process.platform === "darwin" ? "macos" : process.platform === "linux" ? "linux" : process.platform === "win32" ? "win.exe" : null;
  if (!host) throw new Error(`Flashlight managed install is unsupported on ${process.platform}`);
  const zip = join(layout.downloads, `flashlight-${config.flashlightVersion}-${host}.zip`);
  const url = `https://github.com/bamlab/flashlight/releases/download/v${config.flashlightVersion}/flashlight-${host}.zip`;
  if (!await exists(zip)) await download(url, zip);
  await installArchive({ archive: zip, target: layout.flashlightRoot, type: "zip" });
}

async function installJre(config, layout) {
  const asset = await adoptiumAsset(config.jreVersion);
  const archiveType = process.platform === "win32" ? "zip" : "tar.gz";
  const archive = join(layout.downloads, `jre-${config.jreVersion}.${archiveType}`);
  if (!await exists(archive)) await download(asset.url, archive);
  await verify(archive, asset.checksum);
  await installArchive({ archive, target: layout.javaRoot, type: process.platform === "win32" ? "zip" : "tar" });
}

async function installAndroidPlatformTools(config, layout) {
  const descriptor = config.androidPlatformTools.archives[process.platform];
  if (!descriptor) throw new Error(`Android platform tools are unsupported on ${process.platform}`);
  const archive = join(layout.downloads, descriptor.file);
  if (!await exists(archive)) await download(`${config.androidPlatformTools.baseUrl}/${descriptor.file}`, archive);
  await verify(archive, descriptor.sha1, "sha1");
  await installArchive({ archive, target: layout.androidToolsRoot, type: "zip" });
}

export async function setupTools(cwd, config, { onStatus = () => {} } = {}) {
  const layout = toolLayout(cwd, config);
  await mkdir(layout.downloads, { recursive: true });
  let tools = await resolveTools(cwd, config);

  if (!tools.java) {
    onStatus(`Downloading managed Java ${config.jreVersion} runtime`);
    await installJre(config, layout);
  }
  if (!tools.maestro) {
    onStatus(`Downloading Maestro ${config.maestroVersion}`);
    await installMaestro(cwd, config, layout);
  }
  if (!tools.flashlight) {
    onStatus(`Downloading Flashlight ${config.flashlightVersion}`);
    await installFlashlight(config, layout);
  }
  if (!tools.adb) {
    onStatus(`Downloading Android Platform Tools ${config.androidPlatformTools.version}`);
    await installAndroidPlatformTools(config, layout);
  }

  tools = await resolveTools(cwd, config);
  for (const path of [tools.java, tools.maestro, tools.flashlight, tools.adb].filter(Boolean)) await chmod(path, 0o755);
  const manifest = {
    installedAt: new Date().toISOString(),
    maestroVersion: config.maestroVersion,
    maestroSource: tools.maestroSource,
    flashlightVersion: config.flashlightVersion,
    jreVersion: config.jreVersion,
    androidPlatformToolsVersion: config.androidPlatformTools.version,
    sources: ["Eclipse Adoptium", "mobile-dev-inc/Maestro", "bamlab/flashlight"],
  };
  await writeFile(join(layout.root, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
  return tools;
}
