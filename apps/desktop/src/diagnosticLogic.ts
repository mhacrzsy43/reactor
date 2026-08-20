import type { NormalizedResult, SourceMapEvidence } from "./types.ts";

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

export function isUsableDiagnosticResult(result: NormalizedResult): boolean {
  return !result.source.synthetic && result.summary.successfulIterationCount > 0;
}

export function diagnosticContextKey(flowHash: string | undefined, framework: string) {
  return `${flowHash ?? "unbound"}:${framework}`;
}

export function sourceMapStatus(sourceMap: SourceMapEvidence) {
  if (sourceMap.state === "loading") return "正在应用 Source Map";
  if (sourceMap.state === "error") return "Source Map 应用失败";
  if (sourceMap.state === "not-collected") return "尚未导入 Source Map";
  return sourceMap.mappedCount > 0 ? `${sourceMap.mappedCount} 个位置已映射` : "Source Map 已加载，0 个位置可映射";
}
