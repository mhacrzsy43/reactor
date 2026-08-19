import { spawn } from "node:child_process";
import { access } from "node:fs/promises";
import { constants } from "node:fs";
import { delimiter, join } from "node:path";

export function run(command, args, { cwd = process.cwd(), capture = false, env = process.env } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd,
      stdio: capture ? ["ignore", "pipe", "pipe"] : "inherit",
      shell: false,
      env,
    });

    let stdout = "";
    let stderr = "";
    if (capture) {
      child.stdout.on("data", (chunk) => { stdout += chunk; });
      child.stderr.on("data", (chunk) => { stderr += chunk; });
    }

    child.once("error", reject);
    child.once("exit", (code, signal) => {
      if (code === 0) {
        resolve({ stdout, stderr });
      } else {
        reject(new Error(`${command} exited with ${code ?? signal}${stderr ? `: ${stderr.trim()}` : ""}`));
      }
    });
  });
}

export async function commandExists(command) {
  const directories = (process.env.PATH ?? "").split(delimiter).filter(Boolean);
  const extensions = process.platform === "win32"
    ? (process.env.PATHEXT ?? ".EXE;.CMD;.BAT;.COM").split(";")
    : [""];
  for (const directory of directories) {
    for (const extension of extensions) {
      const candidate = join(directory, process.platform === "win32" ? `${command}${extension.toLowerCase()}` : command);
      try {
        await access(candidate, process.platform === "win32" ? constants.F_OK : constants.X_OK);
        return true;
      } catch {
        // Try the next PATH entry.
      }
    }
  }
  return false;
}
