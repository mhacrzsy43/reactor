import { mkdir, readFile, writeFile } from "node:fs/promises";
import { basename, dirname, resolve } from "node:path";

function escapeHtml(value) {
  return String(value ?? "—")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function metric(value, suffix = "") {
  return value === null || value === undefined ? "—" : `${value}${suffix}`;
}

export function renderHtml(results) {
  const rows = results.map((result) => {
    const s = result.summary;
    return `<tr>
      <td><strong>${escapeHtml(result.framework)}</strong><small>${escapeHtml(result.platform)} · ${escapeHtml(result.scenario)} · ${escapeHtml(result.adapter)}</small></td>
      <td>${metric(s.fpsMean)}</td><td>${metric(s.fpsP10)}</td><td>${metric(s.lowFpsSamplePct, "%")}</td>
      <td>${metric(s.cpuMeanPct, "%")}</td><td>${metric(s.ramMeanMb, " MB")}</td><td>${metric(s.ramPeakMb, " MB")}</td>
      <td>${s.successfulIterationCount}/${s.iterationCount}</td>
    </tr>`;
  }).join("\n");
  const warnings = results.flatMap((result) => result.warnings.map((warning) => `${result.framework}: ${warning}`));

  return `<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Reactor Report</title><style>
:root{color-scheme:light dark;--bg:#f5f7fb;--card:#fff;--text:#172033;--muted:#667085;--line:#e5e7eb;--accent:#5b5bd6}
@media(prefers-color-scheme:dark){:root{--bg:#0e1117;--card:#171b24;--text:#f2f4f7;--muted:#98a2b3;--line:#2a3040;--accent:#9b9bff}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--text);font:15px/1.5 system-ui,sans-serif}main{max-width:1120px;margin:48px auto;padding:0 20px}h1{font-size:32px;margin:0 0 8px}p{color:var(--muted);margin:0 0 24px}.card{background:var(--card);border:1px solid var(--line);border-radius:16px;overflow:auto;box-shadow:0 12px 30px #0000000d}table{width:100%;border-collapse:collapse;min-width:900px}th,td{text-align:right;padding:14px 16px;border-bottom:1px solid var(--line);white-space:nowrap}th:first-child,td:first-child{text-align:left}th{font-size:12px;color:var(--muted);text-transform:uppercase;letter-spacing:.04em}small{display:block;color:var(--muted);font-weight:400}.note{margin-top:20px;padding:16px;border-left:4px solid var(--accent);background:var(--card);border-radius:8px}.note p{margin:4px 0}.foot{margin-top:20px;font-size:13px}
</style></head><body><main><h1>Reactor 性能报告</h1><p>同设备、同构建模式、同刷新率和同自动化脚本下的结果才可横向比较。</p>
<div class="card"><table><thead><tr><th>框架</th><th>平均 FPS</th><th>P10 FPS</th><th>低 FPS 样本</th><th>CPU</th><th>平均内存</th><th>P95 峰值内存</th><th>有效轮次</th></tr></thead><tbody>${rows}</tbody></table></div>
${warnings.length ? `<div class="note"><strong>警告</strong>${warnings.map((w) => `<p>${escapeHtml(w)}</p>`).join("")}</div>` : ""}
<p class="foot">低 FPS 样本阈值为设备刷新率的 90%。CPU 为采样线程 CPU 之和，可能超过 100%。</p></main></body></html>`;
}

export async function writeReport(inputFiles, outputFile) {
  const results = await Promise.all(inputFiles.map(async (file) => JSON.parse(await readFile(resolve(file), "utf8"))));
  await mkdir(dirname(resolve(outputFile)), { recursive: true });
  await writeFile(resolve(outputFile), renderHtml(results), "utf8");
  return { outputFile: resolve(outputFile), count: results.length, inputs: inputFiles.map(basename) };
}
