export type Platform = "android" | "ios";

export interface Selector {
  semanticId?: string;
  accessibilityId?: string;
  text?: string;
  enabled?: boolean;
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
  | { action: "input_text"; target: Selector; value: InputValue; clearBefore: boolean }
  | { action: "swipe"; direction: "up" | "down" | "left" | "right"; duration_ms: number }
  | { action: "wait_for"; target: Selector; timeout_ms: number }
  | { action: "assert_visible"; target: Selector }
  | { action: "pause"; duration_ms: number }
  | { action: "repeat"; times: number; steps: FlowStep[] };

export type InputValue =
  | string
  | { variableRef: string }
  | { secretRef: string }
  | { promptRef: string }
  | { totpRef: string };

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
  inputBindings: Array<{ path: string; environmentKey: string; value: InputValue }>;
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
  packageName?: string;
  bounds: InspectorBounds;
  enabled: boolean;
  clickable: boolean;
  editable: boolean;
  password: boolean;
  focused: boolean;
  className?: string;
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

export interface DeviceReplayFrame {
  platform: Platform;
  deviceId: string;
  screenshotDataUrl: string;
  screenshotWidth: number;
  screenshotHeight: number;
  capturedAt: string;
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
  memoryLeak?: AndroidMemoryLeakReport;
  rnDiagnostics?: ReactNativeDiagnosticsSummary;
  warnings: string[];
}

export interface ReactNativeDiagnosticsSummary {
  schemaVersion: number;
  collector: string;
  benchmarkMode?: string;
  eventFile: string;
  eventCount: number;
  componentNames: string[];
  componentRenderCount: number;
  componentTreeCommitCount: number;
  profileCommitCount: number;
  consoleEventCount: number;
  networkEventCount: number;
  hermesHeapSampleCount: number;
  allocatedObjectCount: number;
  retainedObjectCount: number;
  retainedBytes: number;
  profileFile?: string;
  hermesHeapStatsFile?: string;
  hermesHeapSnapshotFile?: string;
  javaHeapDumpFile?: string;
  recentEvents: ReactNativeDiagnosticEvent[];
  warnings: string[];
}

export interface ReactNativeDiagnosticEvent {
  timestampMs: number;
  kind: string;
  payload: Record<string, unknown>;
}

export interface AndroidMemoryCheckpoint {
  kind: "cycle" | "cooldown" | string;
  cycle: number;
  elapsedMs: number;
  cpuPct?: number;
  pssMb?: number;
  rssMb?: number;
  javaHeapMb?: number;
  nativeHeapMb?: number;
}

export interface AndroidMemoryLeakReport {
  schemaVersion: number;
  definitionsVersion: string;
  collector: string;
  cycles: number;
  checkpointEvery: number;
  warmupCycles: number;
  stabilizationMs: number;
  cooldownMs: number;
  slopeMbPerCycle?: number;
  endDeltaMb?: number;
  monotonicGrowthPct?: number;
  cooldownRecoveryMb?: number;
  thresholdMbPerCycle: number;
  verdict: "stable" | "suspected_leak" | "confirmed_leak" | "insufficient_evidence" | string;
  confidence: "low" | "medium" | "high" | string;
  nativeHeapTraceFile?: string;
  nativeRetainedBytes?: number;
  nativeRetainedAllocationCount?: number;
  retentionEvidence?: string;
  managedRetainedObjectCount?: number;
  managedRetainedBytes?: number;
  checkpoints: AndroidMemoryCheckpoint[];
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
  jobId?: string;
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
  source: {
    name?: string;
    status?: string;
    rawFile?: string;
    synthetic: boolean;
  };
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

export type ProfileEvidenceKind = "react" | "hermes" | "baseline";
export type EvidenceBusinessState = "loading" | "available" | "not-collected" | "unsupported" | "unverified" | "error";
export type ProfileEvidenceSource = "managed-run" | "local-file";

export interface ProfileEvidence {
  kind: ProfileEvidenceKind;
  source: ProfileEvidenceSource;
  state: EvidenceBusinessState;
  report?: DiagnosticProfileReport;
  json?: string;
  fileName?: string;
  rawFile?: string;
  runId?: string;
  flowHash?: string;
  collector?: string;
  producer?: string;
  producerVersion?: string;
  sameRunVerified: boolean;
  error?: string;
}

export interface SourceMapEvidence {
  state: "not-collected" | "loading" | "available" | "error";
  fileName?: string;
  json?: string;
  mappedCount: number;
  error?: string;
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

export type TimelineTrackKind = "iterations" | "frames" | "react_commits" | "js_samples" | "runtime_events";
export type TimelineAvailabilityState = "available" | "not_collected" | "unsupported" | "failed" | "unavailable";
export type TimelineCorrelationConfidence = "high" | "medium" | "low" | "unavailable";

export interface TimelineRange {
  startMs: number;
  endMs: number;
}

export interface TimelineTrackAvailability {
  kind: TimelineTrackKind;
  trackId?: number;
  state: TimelineAvailabilityState;
  label?: string;
  reason?: string;
  count?: number;
}

export interface DiagnosticManifest {
  schemaVersion: number;
  runId: string;
  range?: TimelineRange;
  tracks: TimelineTrackAvailability[];
  clock?: {
    quality?: "good" | "fair" | "poor" | "unavailable";
    uncertaintyMs?: number;
    reason?: string;
  };
  warnings?: string[];
}

export interface TimelineOverviewBucket {
  startMs: number;
  endMs: number;
  count: number;
  maxDurationMs?: number;
  slowCount?: number;
}

export interface TimelineOverviewTrack {
  kind: TimelineTrackKind;
  buckets: TimelineOverviewBucket[];
}

export interface TimelineOverview {
  range: TimelineRange;
  tracks: TimelineOverviewTrack[];
}

export interface TimelineItem {
  id: number;
  trackId: number;
  itemType: string;
  track: TimelineTrackKind;
  startMs: number;
  endMs: number;
  label: string;
  detail?: string;
  durationMs?: number;
  severity?: "normal" | "slow" | "warning" | "error";
  data?: Record<string, unknown>;
}

export interface TimelineWindow {
  range: TimelineRange;
  items: TimelineItem[];
  truncated?: boolean;
  warnings?: string[];
}

export interface DiagnosticCorrelationCandidate {
  id?: string;
  itemId?: string;
  track?: TimelineTrackKind;
  label: string;
  relation?: "overlaps" | "adjacent_before" | "adjacent_after" | "contains" | "contained_by" | "unavailable";
  confidence: TimelineCorrelationConfidence;
  overlapRatio?: number;
  gapMs?: number;
  reasons: string[];
}

export interface DiagnosticSelectionAnalysis {
  range: TimelineRange;
  summary?: string;
  eventCount?: number;
  frameCount?: number;
  slowFrameCount?: number;
  reactCommitCount?: number;
  cpuSampleCount?: number;
  topFunctions?: Array<{ name: string; value: number; count: number }>;
  topComponents?: Array<{ name: string; value: number; count: number }>;
  availability?: Partial<Record<TimelineTrackKind, TimelineTrackAvailability>>;
  correlations: DiagnosticCorrelationCandidate[];
  warnings?: string[];
}

export interface FrameDrilldown {
  available: boolean;
  reason?: string;
  frameId?: number;
  startMs?: number;
  endMs?: number;
  durationMs?: number;
  budgetMs?: number;
  classification?: string;
  details?: Array<{ label: string; value: string }>;
  correlations: DiagnosticCorrelationCandidate[];
  warnings?: string[];
}
