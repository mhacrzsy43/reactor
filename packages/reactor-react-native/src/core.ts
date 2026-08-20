export type ReactorProfilerOnRenderCallback = (
  id: string,
  phase: "mount" | "update" | "nested-update",
  actualDuration: number,
  baseDuration: number,
  startTime: number,
  commitTime: number,
) => void;

export type ReactorCapabilityAvailability = {
  status: 'available' | 'unavailable';
  reason?: string;
  classification?: string;
};

export type ReactorDiagnosticsSandboxPaths = {
  root: string;
  events: string;
  reactDevToolsProfile: string;
  hermesHeapStats: string;
  hermesHeapSnapshot: string;
  hermesCpuProfile: string;
};

export type ReactorDiagnosticsCapabilities = {
  diagnosticBuild: boolean;
  sdkVersion: string;
  protocolVersion: number;
  capabilities: string[];
  sandboxPaths?: ReactorDiagnosticsSandboxPaths;
  availability?: Record<string, ReactorCapabilityAvailability>;
};

export type ReactorDiagnosticsBridge = Partial<ReactorDiagnosticsCapabilities> & {
  reset?: () => void;
  appendEvent?: (kind: string, payloadJson: string) => void;
  captureHermesHeap?: (label: string, snapshot: boolean) => void;
};

export type DiagnosticsRuntime = {
  bridge?: ReactorDiagnosticsBridge;
  fetch?: typeof fetch;
  setFetch?: (instrumented: typeof fetch) => void;
  console?: Pick<Console, 'log' | 'warn' | 'error'>;
};

const SDK_VERSION = '1.0.0';
const PROTOCOL_VERSION = 1;
let installed = false;
let runtime: DiagnosticsRuntime = {};

export function configureReactorDiagnostics(next: DiagnosticsRuntime) {
  runtime = next;
}

export function getReactorDiagnosticsCapabilities(): ReactorDiagnosticsCapabilities {
  const bridge = runtime.bridge;
  return {
    diagnosticBuild: bridge?.diagnosticBuild === true,
    sdkVersion: bridge?.sdkVersion ?? SDK_VERSION,
    protocolVersion: bridge?.protocolVersion ?? PROTOCOL_VERSION,
    capabilities: Array.isArray(bridge?.capabilities) ? [...bridge.capabilities] : [],
    sandboxPaths: bridge?.sandboxPaths ? {...bridge.sandboxPaths} : undefined,
    availability: bridge?.availability ? {...bridge.availability} : undefined,
  };
}

function append(kind: string, payload: Record<string, unknown>) {
  try {
    runtime.bridge?.appendEvent?.(kind, JSON.stringify(payload));
  } catch {
    // Diagnostics must never change application behavior or benchmark success.
  }
}

function safeText(value: unknown) {
  if (typeof value === 'string') return value.slice(0, 500);
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  return Object.prototype.toString.call(value);
}

function safeUrl(value: unknown) {
  const raw = typeof value === 'string' ? value : value instanceof Request ? value.url : String(value);
  return raw.split('?')[0].slice(0, 1000);
}

export function resetReactorDiagnostics() {
  runtime.bridge?.reset?.();
  append('session', {event: 'reset', clock: 'wall+elapsedRealtime'});
}

export function installReactorDiagnostics() {
  if (installed) return;
  installed = true;
  const targetConsole = runtime.console;
  if (targetConsole) {
    (['log', 'warn', 'error'] as const).forEach(level => {
      const original = targetConsole[level].bind(targetConsole);
      targetConsole[level] = (...values: unknown[]) => {
        append('console', {level, values: values.map(safeText)});
        original(...values);
      };
    });
  }
  const originalFetch = runtime.fetch;
  if (!originalFetch) return;
  const instrumented: typeof fetch = async (input, init) => {
    const startedAt = Date.now();
    const method = init?.method ?? (input instanceof Request ? input.method : 'GET');
    const url = safeUrl(input);
    append('network', {event: 'start', method, url});
    try {
      const response = await originalFetch(input, init);
      append('network', {
        event: 'complete',
        method,
        url,
        status: response.status,
        durationMs: Date.now() - startedAt,
      });
      return response;
    } catch (error) {
      append('network', {
        event: 'failed',
        method,
        url,
        durationMs: Date.now() - startedAt,
        error: safeText(error),
      });
      throw error;
    }
  };
  runtime.fetch = instrumented;
  runtime.setFetch?.(instrumented);
}

export type FlowMarkerBoundary = 'start' | 'end' | 'failed' | 'cancelled';

export type FlowMarker = {
  boundary: FlowMarkerBoundary;
  entityType: 'iteration' | 'step';
  iterationId: string;
  stepId?: string;
  stepPath?: string;
  action?: string;
  state?: 'completed' | 'failed' | 'cancelled' | 'open';
  source?: string;
  clock?: string;
  uncertaintyMs?: number;
};

export function recordFlowMarker(marker: FlowMarker) {
  append('flow_marker', {
    ...marker,
    source: marker.source ?? 'reactor-rn-sdk',
    clock: marker.clock ?? 'native_elapsed_realtime',
    uncertaintyMs: marker.uncertaintyMs ?? 1,
  });
}

export function recordComponent(name: string, parent?: string, detail?: Record<string, unknown>) {
  append('component_render', {name, parent, ...detail});
}

export function recordBenchmarkMode(mode: string) {
  append('benchmark_mode', {mode});
}

export const recordProfilerCommit: ReactorProfilerOnRenderCallback = (
  id,
  phase,
  actualDuration,
  baseDuration,
  startTime,
  commitTime,
) => {
  append('react_profile', {id, phase, actualDuration, baseDuration, startTime, commitTime});
};

export function recordObjectLifecycle(
  objectId: string,
  action: 'allocate' | 'release' | 'retain',
  bytes: number,
) {
  append('object_lifecycle', {objectId, action, bytes});
}

export function recordHermesHeap(label: string, snapshot = false) {
  try {
    const hermes = (
      globalThis as typeof globalThis & {
        HermesInternal?: {getInstrumentedStats?: () => Record<string, unknown>};
      }
    ).HermesInternal;
    const raw = hermes?.getInstrumentedStats?.();
    if (raw) {
      const stats = Object.fromEntries(
        Object.entries(raw)
          .filter(
            ([key, value]) =>
              /heap|malloc|alloc|gc|object/i.test(key) &&
              (typeof value === 'number' || typeof value === 'string'),
          )
          .slice(0, 64),
      );
      append('hermes_heap', {label, stats, source: 'HermesInternal', informationalOnly: true});
    }
    if (runtime.bridge?.diagnosticBuild) {
      runtime.bridge.captureHermesHeap?.(label, snapshot);
      append('hermes_heap', {label, snapshot, source: 'JSI instrumentation'});
    }
  } catch {
    // Hermes statistics are optional and never change application behavior.
  }
}
