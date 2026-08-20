import { Channel, invoke } from "@tauri-apps/api/core";
import { conservativeAndroidDiagnosticPlan } from "./diagnosticLogic";
import type {
  Bootstrap,
  AnalysisExplanation,
  AnalysisReport,
  JobAnalysis,
  CompiledFlow,
  DemoOutput,
  DiagnosticManifest,
  DiagnosticArtifactRef,
  DiagnosticProfileReport,
  DiagnosticRerunEligibility,
  DiagnosticRunPage,
  DiagnosticSelectionAnalysis,
  DeviceInspectorSnapshot,
  DeviceReplayFrame,
  DiagnosticPlanV1,
  FrameDrilldown,
  Flow,
  FlowChange,
  FlowLock,
  FlowStep,
  GeneratedFlow,
  Job,
  JobPage,
  JobSnapshot,
  NormalizedResult,
  Platform,
  ProfileDiffReport,
  RedactedUiContext,
  TrialPreparation,
  TimelineOverview,
  TimelineRange,
  TimelineWindow,
} from "./types";

export interface GenerateInput {
  intent: string;
  appId: string;
  platform: Platform;
  uiTree?: string;
  endpoint?: string;
  apiKey?: string;
  saveApiKey?: boolean;
  useSavedApiKey?: boolean;
  model?: string;
  provider: "local" | "codex" | "claude" | "cloud";
  cliExecutable?: string;
  projectRoot?: string;
}

export interface FlowModificationProposal {
  generated: GeneratedFlow;
  changes: FlowChange[];
  answer?: string;
}

export interface FlowAssistantDecision {
  kind: "question" | "change";
  answer: string;
}

export async function classifyFlowRequest(input: Omit<GenerateInput, "intent"> & { flow?: Flow; instruction: string }): Promise<FlowAssistantDecision> {
  if (!inTauri) return { kind: input.flow ? "question" : "change", answer: input.flow ? "请在 Reactor 桌面应用中使用 Flow AI。" : "创建 Flow" };
  return invoke("classify_flow_request", { input });
}

export interface TrialLivePerformanceSample {
  source?: string;
  elapsedMs?: number;
  cpuPct?: number;
  pssMb?: number;
  rssMb?: number;
  javaHeapMb?: number;
  nativeHeapMb?: number;
  rn?: {
    sampledEventCount?: number;
    componentRenderCount?: number;
    duplicateComponentRenderCount?: number;
    componentTreeCommitCount?: number;
    profileCommitCount?: number;
    slowestCommitMs?: number;
    slowestCommitName?: string;
    consoleEventCount?: number;
    networkEventCount?: number;
    hermesHeapSampleCount?: number;
  };
  officialMetric?: boolean;
}

export interface CliProviderStatus {
  kind: "codex" | "claude-code";
  label: string;
  available: boolean;
  executable?: string;
  version?: string;
  authenticated: boolean;
  detail: string;
}

export interface LocalModelStatus {
  available: boolean;
  endpoint: string;
  models: string[];
  detail: string;
}

export interface ResourcePolicyView {
  pluginContractVersion: number;
  externalPluginsEnabled: boolean;
  trustedBuiltInAdapters: string[];
  aiCliTimeoutSeconds: number;
  aiCliStdoutBytes: number;
  aiCliStderrBytes: number;
  maxProfileJsonBytes: number;
  maxSourceMapBytes: number;
  localTraceMinFreeBytes: number;
}

export interface MaintenanceStatus {
  schemaVersion: number;
  historyCount: number;
  workspaceBytes: number;
  availableDiskBytes: number;
  sensitiveArtifactCount: number;
  policy: ResourcePolicyView;
  update: {
    currentVersion: string;
    defaultChannel: "stable" | "beta";
    stableEndpoint: string;
    betaEndpoint: string;
    manifestSchemaVersion: number;
    signatureAlgorithm: string;
    signatureRequired: boolean;
    productionKeyConfigured: boolean;
    stagedInstall: boolean;
    rollbackOnFailedHealthCheck: boolean;
    compatibilityLine: string;
  };
  lastUpdate?: {
    version: string;
    phase: "staged" | "activating" | "probing" | "healthy" | "rolled_back" | "quarantined";
    createdAt: string;
    error?: string;
  };
}

export interface DiagnosticBundleResult {
  path: string;
  credentialValuesIncluded: boolean;
  screenshotsIncluded: boolean;
  uiTreesIncluded: boolean;
}

export interface PrivacyEraseResult {
  removedFiles: number;
  removedBytes: number;
  credentialsRemoved: boolean;
  fullReset: boolean;
}

export interface StagedUpdate {
  channel: "stable" | "beta";
  version: string;
  transactionPath: string;
  artifactBytes: number;
  restartRequired: boolean;
}

export interface RealRunInput {
  flowLock: FlowLock;
  framework: string;
  scenario: string;
  deviceId: string;
  durationMs: number;
  iterations: number;
  runMode?: "benchmark" | "diagnose";
  diagnosticPlan?: DiagnosticPlanV1;
  manualSession?: boolean;
  leakTest?: {
    cycles: number;
    checkpointEvery: number;
    warmupCycles: number;
    stabilizationMs: number;
    cooldownMs: number;
    thresholdMbPerCycle: number;
  };
}

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export async function bootstrap(): Promise<Bootstrap> {
  if (inTauri) return invoke("bootstrap");
  return {
    workspace: "Reactor browser preview",
    devices: [],
    doctor: {
      ready: true,
      checks: [
        ["maestro", "自动化引擎"],
        ["flashlight", "Android 兼容采集器"],
        ["java", "内置 Java 运行时"],
        ["adb", "Android 设备桥"],
      ].map(([id, label]) => ({ id, label, available: true, managed: true, detail: "Preview" })),
    },
  };
}

export async function generateFlow(input: GenerateInput): Promise<GeneratedFlow> {
  if (inTauri) return invoke("generate_flow", { input });
  throw new Error("AI Flow 生成请在 Reactor 桌面应用中使用");
}

export async function modifyFlow(input: {
  flow: Flow;
  instruction: string;
  failureContext?: string;
  uiTree?: string;
  endpoint?: string;
  apiKey?: string;
  saveApiKey: boolean;
  useSavedApiKey: boolean;
  model?: string;
  provider: "local" | "codex" | "claude" | "cloud";
  cliExecutable?: string;
  projectRoot?: string;
}): Promise<FlowModificationProposal> {
  if (!inTauri) throw new Error("自然语言修改 Flow 请在 Reactor 桌面应用中使用");
  return invoke("modify_flow", { input });
}

export async function probeFlow(input: GenerateInput): Promise<GeneratedFlow> {
  if (!inTauri) throw new Error("逐步 AI 探索请在 Reactor 桌面应用中使用");
  return invoke("probe_flow", { input });
}

export async function previewGenerationContext(input: {
  appId: string;
  platform: Platform;
  deviceId: string;
}): Promise<RedactedUiContext> {
  if (!inTauri) throw new Error("目标界面读取请在 Reactor 桌面应用中使用");
  return invoke("preview_generation_context", { input });
}

export async function captureDeviceInspector(input: {
  platform: Platform;
  deviceId: string;
}): Promise<DeviceInspectorSnapshot> {
  if (!inTauri) throw new Error("Flow Explorer 请在 Reactor 桌面应用中使用");
  return invoke("capture_device_inspector", { input });
}

export async function captureDeviceReplayFrame(input: {
  platform: Platform;
  deviceId: string;
}): Promise<DeviceReplayFrame> {
  if (!inTauri) throw new Error("设备回放预览请在 Reactor 桌面应用中使用");
  return invoke("capture_device_replay_frame", { input });
}

export async function performExplorerStep(input: {
  platform: Platform;
  deviceId: string;
  appId: string;
  step: FlowStep;
  executionPoint?: { x: number; y: number };
  viewportWidth?: number;
  viewportHeight?: number;
  runtimeInput?: string;
}): Promise<DeviceInspectorSnapshot> {
  if (!inTauri) throw new Error("设备交互录制请在 Reactor 桌面应用中使用");
  return invoke("perform_explorer_step", { input });
}

export async function replayRecordedFlow(input: {
  platform: Platform;
  deviceId: string;
  flow: Flow;
  promptValues?: Record<string, string>;
}, onProgress?: (completedStepIndex: number) => void): Promise<DeviceInspectorSnapshot> {
  if (!inTauri) throw new Error("完整 Flow 回放请在 Reactor 桌面应用中使用");
  const channel = new Channel<{ completedStepIndex: number }>();
  channel.onmessage = (message) => onProgress?.(message.completedStepIndex);
  return invoke("replay_recorded_flow", { input, onProgress: channel });
}

export async function saveFlowSecret(reference: string, value: string): Promise<{ reference: string; stored: boolean }> {
  if (!inTauri) throw new Error("Flow Secret 仅保存在 Reactor 桌面应用的系统凭据库中");
  return invoke("save_flow_secret_value", { input: { reference, value } });
}

export async function getFlowSecretStatus(reference: string): Promise<{ reference: string; stored: boolean }> {
  if (!inTauri) return { reference, stored: false };
  return invoke("get_flow_secret_status", { input: { reference } });
}

export async function deleteFlowSecret(reference: string): Promise<{ reference: string; stored: boolean }> {
  if (!inTauri) throw new Error("Flow Secret 仅保存在 Reactor 桌面应用的系统凭据库中");
  return invoke("delete_flow_secret_value", { input: { reference } });
}

export async function doctorCliProviders(input: {
  codexExecutable?: string;
  claudeExecutable?: string;
} = {}): Promise<CliProviderStatus[]> {
  if (inTauri) return invoke("doctor_cli_providers", { input });
  return [
    { kind: "codex", label: "Codex CLI", available: false, authenticated: false, detail: "请在 Reactor 桌面应用中检测" },
    { kind: "claude-code", label: "Claude Code CLI", available: false, authenticated: false, detail: "请在 Reactor 桌面应用中检测" },
  ];
}

export async function doctorLocalModel(endpoint: string): Promise<LocalModelStatus> {
  if (inTauri) return invoke("doctor_local_model", { input: { endpoint } });
  return { available: false, endpoint, models: [], detail: "请在 Reactor 桌面应用中检测" };
}

export async function compileFlowPreview(flow: Flow): Promise<CompiledFlow> {
  if (inTauri) return invoke("compile_flow_preview", { flow });
  return {
    setup: `# Maestro YAML preview is available in Reactor.app\n# Flow: ${flow.id}\n`,
    measured: `# ${flow.measured.length} measured steps\n`,
    teardown: `# ${flow.teardown.length} teardown steps\n`,
    inputBindings: [],
  };
}

export async function trialGeneratedFlow(generated: GeneratedFlow, deviceId?: string, sourceContext?: RedactedUiContext, promptValues: Record<string, string> = {}): Promise<TrialPreparation> {
  if (inTauri) return invoke("trial_generated_flow", { input: { generated, deviceId, sourceContext, promptValues } });
  const bytes = new TextEncoder().encode(JSON.stringify(generated.flow));
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  const flowHash = Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
  return {
    generated,
    trial: {
      schemaVersion: 1,
      mode: "product_tour_validation",
      passed: true,
      flowHash,
      executedAt: new Date().toISOString(),
      synthetic: true,
    },
    changes: [],
    repairAttempts: 0,
    modelCalls: 0,
  };
}

export async function sampleTrialLivePerformance(input: {
  deviceId: string;
  appId: string;
  elapsedMs: number;
}): Promise<TrialLivePerformanceSample> {
  if (!inTauri) throw new Error("实时试跑采样仅在 Reactor 桌面应用中可用");
  return invoke("sample_trial_live_performance", { input });
}

export async function repairFlow(input: {
  preparation: TrialPreparation;
  deviceId: string;
  endpoint: string;
  apiKey?: string;
  saveApiKey: boolean;
  useSavedApiKey: boolean;
  model: string;
  allowModelContext: boolean;
  provider: "local" | "codex" | "claude" | "cloud";
  cliExecutable?: string;
}): Promise<TrialPreparation> {
  if (!inTauri) throw new Error("AI 自愈请在 Reactor 桌面应用中使用");
  return invoke("repair_flow", { input });
}

export async function confirmFlow(preparation: TrialPreparation): Promise<FlowLock> {
  if (inTauri) return invoke("confirm_flow", { preparation });
  if (!preparation.trial) throw new Error("只能锁定已通过试跑的 Flow");
  const generated = preparation.generated;
  return {
    schemaVersion: 1,
    flowHash: preparation.trial.flowHash,
    lockedAt: new Date().toISOString(),
    compilerVersion: "0.1.0-preview",
    generation: {
      provider: generated.provider,
      model: generated.model,
      promptTemplateVersion: generated.promptTemplateVersion,
    },
    trial: preparation.trial,
    flow: generated.flow,
  };
}

export async function runDemo(
  flowLock: FlowLock,
  onUpdate?: (snapshot: JobSnapshot) => void,
): Promise<DemoOutput> {
  if (inTauri) {
    const started = await invoke<{ jobId: string }>("start_demo", { flowLock });
    return waitForJob(started.jobId, onUpdate);
  }
  const jobId = crypto.randomUUID();
  return {
    jobId,
    results: [
      demoResult(jobId, flowLock.flowHash, "react-native", 55.8, 47.2, 42.4, 160),
      demoResult(jobId, flowLock.flowHash, "flutter", 58.7, 52.8, 36.1, 176),
      demoResult(jobId, flowLock.flowHash, "lynx", 57.5, 50.6, 33.8, 143),
    ],
  };
}

export async function runAndroid(
  input: RealRunInput,
  onUpdate?: (snapshot: JobSnapshot) => void,
): Promise<DemoOutput> {
  if (!inTauri) throw new Error("Android 模拟器/设备测量请在 Reactor 桌面应用中运行");
  const normalizedInput = input.runMode === "diagnose" ? input : { ...input, diagnosticPlan: undefined };
  const started = await invoke<{ jobId: string }>("start_android", { input: normalizedInput });
  return waitForJob(started.jobId, onUpdate);
}

export async function runIos(
  input: RealRunInput,
  onUpdate?: (snapshot: JobSnapshot) => void,
): Promise<DemoOutput> {
  if (!inTauri) throw new Error("iOS Simulator 测量请在 Reactor 桌面应用中运行");
  const started = await invoke<{ jobId: string }>("start_ios", { input });
  return waitForJob(started.jobId, onUpdate);
}

export async function refreshDevices(): Promise<Bootstrap> {
  return bootstrap();
}

export async function prepareManagedTools(): Promise<Bootstrap> {
  if (!inTauri) return bootstrap();
  await invoke("setup_tools", {
    input: { offline: false, proxy: null, maestroOverride: null },
  });
  return bootstrap();
}

export async function getMaintenanceStatus(): Promise<MaintenanceStatus> {
  if (!inTauri) {
    return {
      schemaVersion: 2,
      historyCount: 0,
      workspaceBytes: 0,
      availableDiskBytes: 0,
      sensitiveArtifactCount: 0,
      policy: {
        pluginContractVersion: 1,
        externalPluginsEnabled: false,
        trustedBuiltInAdapters: ["maestro", "android-perfetto", "android-flashlight", "ios-xctrace"],
        aiCliTimeoutSeconds: 120,
        aiCliStdoutBytes: 1_048_576,
        aiCliStderrBytes: 262_144,
        maxProfileJsonBytes: 67_108_864,
        maxSourceMapBytes: 134_217_728,
        localTraceMinFreeBytes: 134_217_728,
      },
      update: {
        currentVersion: "0.1.0",
        defaultChannel: "stable",
        stableEndpoint: "https://github.com/mhacrzsy43/reactor/releases/latest/download/stable.json",
        betaEndpoint: "https://github.com/mhacrzsy43/reactor/releases/download/beta/beta.json",
        manifestSchemaVersion: 1,
        signatureAlgorithm: "Ed25519",
        signatureRequired: true,
        productionKeyConfigured: false,
        stagedInstall: true,
        rollbackOnFailedHealthCheck: true,
        compatibilityLine: "1.x keeps Flow v1, Result v1 and transactional database upgrades readable",
      },
      lastUpdate: undefined,
    };
  }
  return invoke("maintenance_status");
}

export async function createDiagnosticBundle(): Promise<DiagnosticBundleResult> {
  if (!inTauri) throw new Error("诊断包请在 Reactor 桌面应用中生成");
  return invoke("create_diagnostic_bundle");
}

export async function stageUpdate(channel: "stable" | "beta"): Promise<StagedUpdate> {
  if (!inTauri) throw new Error("应用更新请在 Reactor 桌面应用中执行");
  return invoke("stage_update", { input: { channel } });
}

export async function installStagedUpdate(transactionPath: string): Promise<void> {
  if (!inTauri) throw new Error("应用更新请在 Reactor 桌面应用中执行");
  await invoke("install_staged_update", { input: { transactionPath } });
}

export async function erasePrivateData(mode: "sensitive_artifacts" | "all_local_data"): Promise<PrivacyEraseResult> {
  if (!inTauri) throw new Error("本地数据擦除请在 Reactor 桌面应用中执行");
  return invoke("erase_private_data", {
    input: {
      mode,
      confirmation: mode === "all_local_data" ? "ERASE ALL" : "ERASE SENSITIVE",
    },
  });
}

export async function openReport(path: string): Promise<void> {
  if (!inTauri) throw new Error("HTML 报告请在 Reactor 桌面应用中打开");
  await invoke("open_report", { path });
}

export async function cancelJob(jobId: string): Promise<Job> {
  if (!inTauri) throw new Error("只能在 Reactor 桌面应用中取消任务");
  return invoke("cancel_job", { jobId });
}

export async function stopManualDiagnose(jobId: string): Promise<Job> {
  if (!inTauri) throw new Error("手动诊断录制只能在 Reactor 桌面应用中停止");
  return invoke("stop_manual_diagnose", { jobId });
}

export async function resumeJob(
  jobId: string,
  onUpdate?: (snapshot: JobSnapshot) => void,
): Promise<DemoOutput> {
  if (!inTauri) throw new Error("只能在 Reactor 桌面应用中恢复任务");
  return waitForJob(jobId, onUpdate);
}

export async function listJobs(limit = 25, offset = 0): Promise<JobPage> {
  if (!inTauri) return { jobs: [], total: 0, limit, offset };
  return invoke("list_jobs", { limit, offset });
}

export async function listDiagnosticRuns(input: {
  limit?: number;
  offset?: number;
  flowHash?: string;
  framework?: string;
} = {}): Promise<DiagnosticRunPage> {
  const query = {
    limit: input.limit ?? 20,
    offset: input.offset ?? 0,
    flowHash: input.flowHash ?? null,
    framework: input.framework ?? null,
  };
  if (!inTauri) return { runs: [], total: 0, limit: query.limit, offset: query.offset };
  return invoke("list_diagnostic_runs", { input: query });
}

export async function getDiagnosticRerunEligibility(jobId: string, runId: string, flowHash?: string): Promise<DiagnosticRerunEligibility> {
  if (!inTauri) return { jobId, runId, eligible: false, reason: "请在 Reactor 桌面应用中重新运行", lockAvailable: false, platform: "unknown", diagnoseAvailable: false };
  return invoke("get_diagnostic_rerun_eligibility", { input: { jobId, runId, flowHash: flowHash ?? null } });
}

export async function loadHistoricalFlowLock(jobId: string, runId: string, flowHash?: string): Promise<FlowLock | null> {
  if (!inTauri) return null;
  return invoke("load_historical_flow_lock", { input: { jobId, runId, flowHash: flowHash ?? null } });
}

export async function runAndroidDiagnose(
  input: RealRunInput,
  onUpdate?: (snapshot: JobSnapshot) => void,
): Promise<DemoOutput> {
  const iterations = Math.max(1, Math.min(3, Math.trunc(input.iterations)));
  const diagnosticPlan = input.diagnosticPlan ?? conservativeAndroidDiagnosticPlan(input.durationMs, iterations);
  const durationMs = Math.min(input.durationMs, Math.floor(diagnosticPlan.resourceLimits.maxDurationMs / iterations));
  return runAndroid({ ...input, durationMs, iterations, runMode: "diagnose", diagnosticPlan }, onUpdate);
}

export async function runAndroidManualDiagnose(
  input: RealRunInput,
  onUpdate?: (snapshot: JobSnapshot) => void,
): Promise<DemoOutput> {
  const durationMs = Math.max(1_000, Math.min(5 * 60 * 1_000, Math.trunc(input.durationMs)));
  const diagnosticPlan = input.diagnosticPlan ?? conservativeAndroidDiagnosticPlan(durationMs, 1);
  return runAndroid({
    ...input,
    scenario: "manual-diagnose",
    durationMs,
    iterations: 1,
    runMode: "diagnose",
    diagnosticPlan,
    leakTest: undefined,
    manualSession: true,
  }, onUpdate);
}

export async function analyzeJobPair(baselineJobId: string, currentJobId: string): Promise<JobAnalysis> {
  if (!inTauri) throw new Error("结果分析请在 Reactor 桌面应用中使用");
  return invoke("analyze_job_pair", {
    input: { baselineJobId, currentJobId, policy: null },
  });
}

export async function explainAnalysis(input: {
  report: AnalysisReport;
  provider: "offline" | "local" | "codex" | "claude" | "cloud";
  endpoint?: string;
  apiKey?: string;
  saveApiKey?: boolean;
  useSavedApiKey?: boolean;
  model?: string;
  cliExecutable?: string;
}): Promise<AnalysisExplanation> {
  if (!inTauri) throw new Error("AI 结果解读请在 Reactor 桌面应用中使用");
  return invoke("explain_analysis", { input });
}

export async function analyzeProfileJson(json: string, sourceMap?: string): Promise<DiagnosticProfileReport> {
  if (!inTauri) throw new Error("Profile 诊断请在 Reactor 桌面应用中使用");
  return invoke("analyze_profile_json", { input: { json, sourceMap: sourceMap ?? null } });
}

export async function analyzeManagedProfile(jobId: string, runId: string, artifact: DiagnosticArtifactRef): Promise<DiagnosticProfileReport> {
  if (!inTauri) throw new Error("受管 Profile 诊断请在 Reactor 桌面应用中使用");
  return invoke("analyze_managed_profile", {
    input: {
      jobId,
      runId,
      artifact: { path: artifact.path, sizeBytes: artifact.sizeBytes, sha256: artifact.sha256 },
    },
  });
}

export async function diffProfileReports(
  baseline: DiagnosticProfileReport,
  current: DiagnosticProfileReport,
): Promise<ProfileDiffReport> {
  if (!inTauri) throw new Error("Profile 对比请在 Reactor 桌面应用中使用");
  return invoke("diff_profile_reports", { input: { baseline, current } });
}

export async function getDiagnosticManifest(jobId: string, runId: string): Promise<DiagnosticManifest> {
  if (!inTauri) throw new Error("统一时间线请在 Reactor 桌面应用中查看");
  return invoke("get_diagnostic_manifest", { input: { jobId, runId, flowHash: null } });
}

export async function getTimelineOverview(
  jobId: string,
  runId: string,
  range: TimelineRange,
  pixelWidth: number,
): Promise<TimelineOverview> {
  if (!inTauri) throw new Error("统一时间线请在 Reactor 桌面应用中查看");
  return invoke("get_timeline_overview", { input: { jobId, runId, startMs: range.startMs, endMs: range.endMs, pixelWidth } });
}

export async function getTimelineWindow(
  jobId: string,
  runId: string,
  range: TimelineRange,
  trackIds: number[],
): Promise<TimelineWindow> {
  if (!inTauri) throw new Error("统一时间线请在 Reactor 桌面应用中查看");
  return invoke("get_timeline_window", { input: { jobId, runId, startMs: range.startMs, endMs: range.endMs, trackIds } });
}

export async function analyzeDiagnosticSelection(
  jobId: string,
  runId: string,
  range: TimelineRange,
): Promise<DiagnosticSelectionAnalysis> {
  if (!inTauri) throw new Error("时间段分析请在 Reactor 桌面应用中使用");
  return invoke("analyze_diagnostic_selection", { input: { jobId, runId, startMs: range.startMs, endMs: range.endMs } });
}

export async function getFrameDrilldown(jobId: string, runId: string, frameId: number): Promise<FrameDrilldown> {
  if (!inTauri) throw new Error("帧下钻请在 Reactor 桌面应用中使用");
  return invoke("get_frame_drilldown", { input: { jobId, runId, frameId } });
}

export async function getJobSnapshot(
  jobId: string,
  query: { cursor?: number; before?: number; limit?: number } = {},
): Promise<JobSnapshot> {
  if (!inTauri) throw new Error("运行历史请在 Reactor 桌面应用中查看");
  const before = query.before ?? (query.cursor === undefined ? Number.MAX_SAFE_INTEGER : undefined);
  return invoke("get_job", {
    jobId,
    cursor: query.cursor,
    before,
    eventLimit: query.limit ?? 100,
  });
}

export const JOB_POLL_INTERVAL_MS = 500;

async function waitForJob(
  jobId: string,
  onUpdate?: (snapshot: JobSnapshot) => void,
): Promise<DemoOutput> {
  let cursor = 0;
  let events: JobSnapshot["events"] = [];
  for (;;) {
    const update = await invoke<JobSnapshot>("get_job", { jobId, cursor, eventLimit: 100 });
    events = [...events, ...update.events];
    cursor = events.at(-1)?.id ?? cursor;
    const snapshot = { ...update, events };
    onUpdate?.(snapshot);
    if (update.hasMoreEvents) continue;
    if (snapshot.job.state === "completed") return { jobId, results: snapshot.results, reportPath: snapshot.reportPath };
    if (snapshot.job.state === "failed" || snapshot.job.state === "cancelled") {
      throw new Error(snapshot.job.error ?? `任务已${snapshot.job.state === "failed" ? "失败" : "取消"}`);
    }
    await new Promise((resolve) => window.setTimeout(resolve, JOB_POLL_INTERVAL_MS));
  }
}

function demoResult(
  jobId: string,
  flowHash: string,
  framework: string,
  fps: number,
  p10: number,
  cpu: number,
  memory: number,
): NormalizedResult {
  return {
    runId: `${jobId}-${framework}`,
    framework,
    platform: "android",
    scenario: "list",
    adapter: "reactor-synthetic-tour",
    flowHash,
    source: { synthetic: true, status: "SYNTHETIC" },
    summary: {
      iterationCount: 10,
      successfulIterationCount: 10,
      fpsMean: fps,
      fpsP10: p10,
      lowFpsSamplePct: (60 - fps) * 1.7,
      ramMeanMb: memory,
      ramPeakMb: memory + 8,
      cpuMeanPct: cpu,
    },
    warnings: ["模拟数据仅用于体验 Reactor 工作流，不得用于框架性能结论。"],
  };
}
