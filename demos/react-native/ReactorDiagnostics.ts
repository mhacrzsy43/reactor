import {NativeModules} from 'react-native';
import {
  configureReactorDiagnostics,
  installReactorDiagnostics as installCore,
  type ReactorDiagnosticsBridge,
} from '../../packages/reactor-react-native/src';

const bridge = NativeModules.ReactorDiagnostics as ReactorDiagnosticsBridge | undefined;

configureReactorDiagnostics({
  bridge,
  console,
  fetch: globalThis.fetch.bind(globalThis),
  setFetch: instrumented => {
    globalThis.fetch = instrumented;
  },
});

export {
  getReactorDiagnosticsCapabilities,
  recordBenchmarkMode,
  recordComponent,
  recordHermesHeap,
  recordObjectLifecycle,
  recordProfilerCommit,
  resetReactorDiagnostics,
} from '../../packages/reactor-react-native/src';

export function installReactorDiagnostics() {
  installCore();
}
