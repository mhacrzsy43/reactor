export type Platform = "android" | "ios";

export interface Selector {
  semanticId?: string;
  accessibilityId?: string;
  text?: string;
  index?: number;
  coordinate?: { x: number; y: number };
}

export interface Coordinate {
  x: number;
  y: number;
}

export type FlowStep =
  | { action: "reset_app_state" }
  | { action: "launch_app" }
  | { action: "tap"; target: Selector }
  | { action: "input_text"; target: Selector; text: string }
  | { action: "swipe"; direction: "up" | "down" | "left" | "right"; duration_ms: number }
  | { action: "wait_for"; target: Selector; timeout_ms: number }
  | { action: "assert_visible"; target: Selector }
  | { action: "pause"; duration_ms: number }
  | { action: "repeat"; times: number; steps: FlowStep[] };

export interface Flow {
  schemaVersion: number;
  id: string;
  name: string;
  appId: string;
  platform: Platform;
  intent?: string;
  setup: FlowStep[];
  measured: FlowStep[];
  teardown: FlowStep[];
}

export interface GeneratedFlow {
  flow: Flow;
  provider: string;
  model: string;
  promptTemplateVersion: string;
  notes: string[];
}

export interface CompiledFlow {
  setup: string;
  measured: string;
  teardown: string;
}

export interface FlowLock {
  schemaVersion: number;
  flowHash: string;
  lockedAt: string;
  compilerVersion: string;
  generation?: { provider: string; model: string; promptTemplateVersion: string };
  trial?: FlowTrialEvidence;
  flow: Flow;
}

export interface FlowTrialEvidence {
  schemaVersion: number;
  mode: "android_target" | "ios_simulator" | "product_tour_validation";
  passed: boolean;
  flowHash: string;
  executedAt: string;
  deviceId?: string;
  artifactDir?: string;
  synthetic: boolean;
}

export interface ContextPreview {
  originalChars: number;
  includedChars: number;
  elementCount: number;
  redactionCount: number;
  screenshotCount: number;
  screenshotBytesUploaded: number;
  fields: string[];
}

export interface RedactedUiContext {
  uiTree: string;
  preview: ContextPreview;
}

export interface FlowChange {
  path: string;
  before?: unknown;
  after?: unknown;
}

export interface TrialPreparation {
  generated: GeneratedFlow;
  trial?: FlowTrialEvidence;
  failure?: { stepPath: string; code: string; message: string };
  evidence?: {
    artifactDir: string;
    errorPath: string;
    uiTreePath?: string;
    screenshotPath?: string;
  };
  context?: RedactedUiContext;
  sourceContext?: RedactedUiContext;
  goalEvidence?: {
    marker: string;
    sourceContainsMarker: boolean;
    destinationContainsMarker: boolean;
    sourceElements: number;
    destinationElements: number;
    verified: boolean;
  };
  changes: FlowChange[];
  repairAttempts: number;
  modelCalls: number;
  auditPath?: string;
}

export interface DoctorCheck {
  id: string;
  label: string;
  available: boolean;
  managed: boolean;
  detail?: string;
}

export interface DoctorReport {
  ready: boolean;
  checks: DoctorCheck[];
}

export interface Device {
  id: string;
  state: string;
  platform: string;
  name?: string;
  physical: boolean;
  metadata: Record<string, string>;
}

export type SelectorStability = "stable" | "contextual" | "brittle";

export interface InspectorBounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface InspectorSelectorCandidate {
  strategy: string;
  label: string;
  selector: Selector;
  score: number;
  stability: SelectorStability;
  reason: string;
}

export interface InspectorElement {
  key: string;
  depth: number;
  text?: string;
  accessibilityText?: string;
  resourceId?: string;
  bounds: InspectorBounds;
  enabled: boolean;
  clickable: boolean;
  candidates: InspectorSelectorCandidate[];
}

export interface DeviceInspectorSnapshot {
  platform: Platform;
  deviceId: string;
  screenshotDataUrl: string;
  screenshotWidth: number;
  screenshotHeight: number;
  viewportWidth: number;
  viewportHeight: number;
  capturedAt: string;
  elements: InspectorElement[];
  warnings: string[];
}

export interface Bootstrap {
  doctor: DoctorReport;
  devices: Device[];
  workspace: string;
  activeJob?: Job;
}

export interface ResultSummary {
  iterationCount: number;
  successfulIterationCount: number;
  fpsMean?: number;
  fpsP10?: number;
  lowFpsSamplePct?: number;
  ramMeanMb?: number;
  ramPeakMb?: number;
  cpuMeanPct?: number;
}

export interface AndroidNativeMetrics {
  schemaVersion: number;
  definitionsVersion: string;
  collector: string;
  traceProcessorVersion: string;
  perfettoTraceFile: string;
  frameCount: number;
  frameTimeMeanMs?: number;
  frameTimeP50Ms?: number;
  frameTimeP95Ms?: number;
  frameTimeP99Ms?: number;
  jankFrameCount: number;
  jankFramePct?: number;
  overBudgetFramePct?: number;
  startupTimeMs?: number;
  memoryPssMb?: number;
  thermalStatusBefore?: number;
  thermalStatusAfter?: number;
  warnings: string[];
}

export interface IosNativeMetrics {
  schemaVersion: number;
  definitionsVersion: string;
  collector: string;
  xctraceVersion: string;
  template: string;
  traceFile: string;
  traceArchiveFile: string;
  tocExportFile: string;
  profileExportFile: string;
  recordingDurationMs: number;
  cpuSampleCount: number;
  cpuMeanPct?: number;
  frameTimeP95Ms?: number;
  startupTimeMs?: number;
  memoryPeakMb?: number;
  energyImpact?: number;
  availability: {
    cpu: string;
    frames: string;
    startup: string;
    memory: string;
    energy: string;
  };
  warnings: string[];
}

export interface NormalizedResult {
  runId: string;
  framework: string;
  platform: string;
  scenario: string;
  adapter: string;
  flowHash: string;
  appId?: string;
  appVersion?: string;
  device?: {
    id?: string;
    name?: string;
    osVersion?: string;
    physical?: boolean;
    refreshRate?: number;
  };
  source: { synthetic: boolean; status?: string };
  androidNative?: AndroidNativeMetrics;
  iosNative?: IosNativeMetrics;
  summary: ResultSummary;
  warnings: string[];
}

export interface DemoOutput {
  jobId: string;
  results: NormalizedResult[];
  reportPath?: string;
}

export type JobState = "queued" | "preflight" | "warmup" | "measuring" | "normalizing" | "completed" | "failed" | "cancelled";

export interface Job {
  id: string;
  createdAt: string;
  updatedAt: string;
  state: JobState;
  request: unknown;
  error?: string;
  resultPath?: string;
  workerPid?: number;
}

export interface JobEvent {
  id: number;
  jobId: string;
  createdAt: string;
  phase: JobState;
  message: string;
  data?: unknown;
}

export interface JobSnapshot {
  job: Job;
  events: JobEvent[];
  hasMoreEvents: boolean;
  results: NormalizedResult[];
  reportPath?: string;
}

export interface JobPage {
  jobs: Job[];
  total: number;
  offset: number;
  limit: number;
}

export type MetricVerdict = "improved" | "stable" | "regressed" | "unavailable";
export type AnalysisVerdict = "improved" | "stable" | "regressed" | "incompatible";

export interface MetricComparison {
  id: string;
  label: string;
  unit: string;
  direction: "lower_is_better" | "higher_is_better";
  baseline?: number;
  current?: number;
  absoluteDelta?: number;
  percentDelta?: number;
  thresholdPct: number;
  verdict: MetricVerdict;
  evidenceRefs: string[];
}

export interface AnalysisFinding {
  id: string;
  severity: "info" | "warning" | "critical";
  title: string;
  summary: string;
  fact: boolean;
  metricRefs: string[];
  evidenceRefs: string[];
}

export interface AnalysisReport {
  schemaVersion: number;
  verdict: AnalysisVerdict;
  compatibility: { compatible: boolean; reasons: string[]; warnings: string[] };
  metrics: MetricComparison[];
  findings: AnalysisFinding[];
  evidence: {
    schemaVersion: number;
    baselineRunId: string;
    currentRunId: string;
    flowHash: string;
    framework: string;
    platform: string;
    scenario: string;
    deviceClass: string;
    metricDefinitions: string[];
    rawEvidence: string[];
    normalizedFacts: unknown;
  };
}

export interface JobAnalysis {
  baselineJob: Job;
  currentJob: Job;
  reports: AnalysisReport[];
}

export interface CitedInsight {
  title: string;
  text: string;
  fact: boolean;
  metricRefs: string[];
  evidenceRefs: string[];
}

export interface AnalysisExplanation {
  schemaVersion: number;
  verdict: AnalysisVerdict;
  provider: string;
  model: string;
  promptTemplateVersion: string;
  summary: string;
  facts: CitedInsight[];
  hypotheses: CitedInsight[];
  nextSteps: { title: string; text: string }[];
}

export interface SourceLocation {
  file: string;
  line?: number;
  column?: number;
}

export interface ProfileCommit {
  id: string;
  rootId: string;
  index: number;
  timestampMs?: number;
  durationMs?: number;
  renderedComponentIds: string[];
  changedComponentIds: string[];
  updaterComponentIds: string[];
  changes: ComponentChangeEvidence[];
}

export interface ComponentChangeEvidence {
  componentId: string;
  props: string[];
  state: string[];
  context: string[];
  hooks: number[];
  didHooksChange: boolean;
  isFirstMount: boolean;
}

export interface ComponentProfileStat {
  id: string;
  name: string;
  parentId?: string;
  parentName?: string;
  source?: SourceLocation;
  renderCount: number;
  commitCount: number;
  changedRenderCount: number;
  unchangedRenderCount: number;
  updaterCount: number;
  totalTimeMs: number;
  selfTimeMs: number;
  averageTimeMs: number;
  p50TimeMs: number;
  p95TimeMs: number;
  maxTimeMs: number;
  commitIds: string[];
}

export interface FunctionProfileStat {
  id: string;
  name: string;
  source?: SourceLocation;
  sampleCount: number;
  selfTimeMs: number;
  selfTimePct: number;
}

export interface DiagnosticFinding {
  ruleId: string;
  severity: "info" | "warning" | "critical";
  title: string;
  summary: string;
  componentId?: string;
  componentName?: string;
  commitIds: string[];
  evidenceRefs: string[];
  source?: SourceLocation;
}

export interface DiagnosticProfileReport {
  schemaVersion: number;
  profileType: "react_profiler" | "hermes_cpu";
  sourceFormat: string;
  profileId: string;
  rootCount: number;
  commitCount: number;
  totalDurationMs: number;
  components: ComponentProfileStat[];
  functions: FunctionProfileStat[];
  commits: ProfileCommit[];
  findings: DiagnosticFinding[];
  warnings: string[];
  sourceMapApplied: boolean;
  sourceMapMappedCount: number;
}

export interface ComponentProfileDiff {
  key: string;
  name: string;
  source?: SourceLocation;
  baselineRenderCount: number;
  currentRenderCount: number;
  renderCountDelta: number;
  renderCountDeltaPct?: number;
  baselineTotalTimeMs: number;
  currentTotalTimeMs: number;
  totalTimeDeltaMs: number;
  totalTimeDeltaPct?: number;
  regressed: boolean;
  newComponent: boolean;
  removedComponent: boolean;
}

export interface ProfileDiffReport {
  schemaVersion: number;
  compatible: boolean;
  reasons: string[];
  components: ComponentProfileDiff[];
  regressionCount: number;
}
