import type { DiagnosticPlanV1, DiagnosticRunSummary, Flow, FlowStep, InputValue, NormalizedResult, SourceMapEvidence } from "./types.ts";

const MAX_DIAGNOSE_DURATION_MS = 5 * 60 * 1_000;

export function conservativeAndroidDiagnosticPlan(durationMs: number, iterations: number): DiagnosticPlanV1 {
  const boundedIterations = Math.max(1, Math.min(3, Math.trunc(iterations)));
  const boundedDurationMs = Math.max(1, Math.min(Math.trunc(durationMs), Math.floor(MAX_DIAGNOSE_DURATION_MS / boundedIterations)));
  return {
    schemaVersion: 1,
    mode: "in_band",
    collectors: [{ collector: "hermes-cpu", required: false }],
    resourceLimits: {
      maxDurationMs: boundedDurationMs * boundedIterations,
      maxArtifactBytes: 256 * 1024 * 1024,
      maxEvents: 500_000,
      maxSamples: 2_000_000,
    },
  };
}

export function telemetrySlopePerMinute(points: Array<{ timeMs: number; value: number }>) {
  if (points.length < 2) return undefined;
  const origin = points[0].timeMs;
  const xs = points.map((point) => (point.timeMs - origin) / 60_000);
  const meanX = xs.reduce((sum, value) => sum + value, 0) / xs.length;
  const meanY = points.reduce((sum, point) => sum + point.value, 0) / points.length;
  const denominator = xs.reduce((sum, value) => sum + (value - meanX) ** 2, 0);
  if (denominator === 0) return undefined;
  return points.reduce((sum, point, index) => sum + (xs[index] - meanX) * (point.value - meanY), 0) / denominator;
}

export class RequestTokens<Request extends string> {
  private tokens: Record<Request, number>;

  constructor(requests: readonly Request[]) {
    this.tokens = Object.fromEntries(requests.map((request) => [request, 0])) as Record<Request, number>;
  }

  start(request: Request) {
    this.tokens[request] += 1;
    return this.tokens[request];
  }

  isCurrent(request: Request, token: number) {
    return this.tokens[request] === token;
  }

  cancel(request: Request) {
    this.tokens[request] += 1;
  }

  cancelAll() {
    for (const request of Object.keys(this.tokens) as Request[]) this.cancel(request);
  }
}

export function historicalRerunBlockingReferences(flow: Flow): string[] {
  const references = new Set<string>();
  const inspectValue = (value: InputValue) => {
    if (typeof value === "string") return;
    if ("promptRef" in value) references.add("promptRef");
    else if ("secretRef" in value) references.add("secretRef");
    else if ("totpRef" in value) references.add("totpRef");
  };
  const inspectSteps = (steps: FlowStep[]) => {
    for (const step of steps) {
      if (step.action === "input_text") inspectValue(step.value);
      else if (step.action === "repeat") inspectSteps(step.steps);
    }
  };
  inspectSteps(flow.setup);
  inspectSteps(flow.measured);
  inspectSteps(flow.teardown);
  return [...references].sort();
}

export function isUsableDiagnosticResult(result: NormalizedResult): boolean {
  return !result.source.synthetic && result.summary.successfulIterationCount > 0;
}

export function diagnosticWorkbenchKey(jobId: string | undefined, runId: string | undefined, flowHash: string | undefined, framework: string) {
  return [jobId ?? "no-job", runId ?? "no-run", flowHash ?? "unbound", framework].join(":");
}

/** @deprecated Prefer diagnosticWorkbenchKey for historical Run workbenches. */
export function diagnosticContextKey(flowHash: string | undefined, framework: string) {
  return diagnosticWorkbenchKey(undefined, undefined, flowHash, framework);
}

export function groupDiagnosticRunsByFlow(runs: readonly DiagnosticRunSummary[]) {
  const groups = new Map<string, DiagnosticRunSummary[]>();
  for (const run of runs) {
    const group = groups.get(run.flowHash) ?? [];
    group.push(run);
    groups.set(run.flowHash, group);
  }
  return [...groups.entries()].map(([flowHash, flowRuns]) => ({
    flowHash,
    runs: [...flowRuns].sort((a, b) => b.createdAt.localeCompare(a.createdAt)),
  }));
}

export function preferredDiagnosticFlowHash(runs: readonly DiagnosticRunSummary[], currentFlowHash?: string) {
  if (currentFlowHash && runs.some((run) => run.flowHash === currentFlowHash)) return currentFlowHash;
  return runs[0]?.flowHash;
}

export function diagnosticRunIdentity(run: Pick<DiagnosticRunSummary, "jobId" | "runId">) {
  return `${run.jobId}:${run.runId}`;
}

export function preferredDiagnosticRun(runs: readonly DiagnosticRunSummary[], flowHash?: string, currentIdentity?: string) {
  const matching = runs.filter((run) => !flowHash || run.flowHash === flowHash);
  return matching.find((run) => diagnosticRunIdentity(run) === currentIdentity) ?? matching[0];
}

export function sourceMapStatus(sourceMap: SourceMapEvidence) {
  if (sourceMap.state === "loading") return "正在应用 Source Map";
  if (sourceMap.state === "error") return "Source Map 应用失败";
  if (sourceMap.state === "not-collected") return "尚未导入 Source Map";
  return sourceMap.mappedCount > 0 ? `${sourceMap.mappedCount} 个位置已映射` : "Source Map 已加载，0 个位置可映射";
}
