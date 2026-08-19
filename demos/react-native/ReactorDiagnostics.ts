import {NativeModules} from 'react-native';
import type {ProfilerOnRenderCallback} from 'react';

type DiagnosticsBridge = {
  diagnosticBuild?: boolean;
  reset?: () => void;
  appendEvent?: (kind: string, payloadJson: string) => void;
  captureHermesHeap?: (label: string, snapshot: boolean) => void;
};

const bridge = NativeModules.ReactorDiagnostics as DiagnosticsBridge | undefined;
let installed = false;

function append(kind: string, payload: Record<string, unknown>) {
  try {
    bridge?.appendEvent?.(kind, JSON.stringify(payload));
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
  bridge?.reset?.();
  append('session', {event: 'reset', clock: 'wall'});
}

export function installReactorDiagnostics() {
  if (installed) return;
  installed = true;
  (['log', 'warn', 'error'] as const).forEach(level => {
    const original = console[level].bind(console);
    console[level] = (...values: unknown[]) => {
      append('console', {level, values: values.map(safeText)});
      original(...values);
    };
  });
  const originalFetch = globalThis.fetch.bind(globalThis);
  const instrumentedFetch: typeof fetch = async (input, init) => {
    const startedAt = Date.now();
    const method = init?.method ?? (input instanceof Request ? input.method : 'GET');
    const url = safeUrl(input);
    append('network', {event: 'start', method, url});
    try {
      const response = await originalFetch(input, init);
      append('network', {event: 'complete', method, url, status: response.status, durationMs: Date.now() - startedAt});
      return response;
    } catch (error) {
      append('network', {event: 'failed', method, url, durationMs: Date.now() - startedAt, error: safeText(error)});
      throw error;
    }
  };
  globalThis.fetch = instrumentedFetch;
}

export function recordComponent(name: string, parent?: string, detail?: Record<string, unknown>) {
  append('component_render', {name, parent, ...detail});
}

export function recordBenchmarkMode(mode: string) {
  append('benchmark_mode', {mode});
}

export const recordProfilerCommit: ProfilerOnRenderCallback = (
  id,
  phase,
  actualDuration,
  baseDuration,
  startTime,
  commitTime,
) => {
  append('react_profile', {id, phase, actualDuration, baseDuration, startTime, commitTime});
};

export function recordObjectLifecycle(objectId: string, action: 'allocate' | 'release' | 'retain', bytes: number) {
  append('object_lifecycle', {objectId, action, bytes});
}

export function recordHermesHeap(label: string, snapshot = false) {
  try {
    const hermes = (globalThis as typeof globalThis & {HermesInternal?: {getInstrumentedStats?: () => Record<string, unknown>}}).HermesInternal;
    const raw = hermes?.getInstrumentedStats?.();
    if (raw) {
      const stats = Object.fromEntries(
        Object.entries(raw)
          .filter(([key, value]) => /heap|malloc|alloc|gc|object/i.test(key) && (typeof value === 'number' || typeof value === 'string'))
          .slice(0, 64),
      );
      append('hermes_heap', {label, stats, source: 'HermesInternal'});
    }
    if (bridge?.diagnosticBuild) {
      bridge.captureHermesHeap?.(label, snapshot);
      append('hermes_heap', {label, snapshot, source: 'JSI instrumentation'});
    }
  } catch {
    // Hermes stats are optional and never change application behavior.
  }
}
