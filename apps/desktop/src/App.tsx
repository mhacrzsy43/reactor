import {
  Activity,
  ArrowRight,
  Bot,
  Check,
  ChevronDown,
  CircleGauge,
  Cpu,
  Database,
  Flame,
  FileDown,
  FlaskConical,
  HardDrive,
  Laptop,
  LockKeyhole,
  Moon,
  Pencil,
  Play,
  RefreshCw,
  Save,
  ScanSearch,
  Settings2,
  ShieldCheck,
  Smartphone,
  Sparkles,
  Sun,
  Trash2,
  WandSparkles,
  Zap,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { JOB_POLL_INTERVAL_MS, analyzeJobPair, bootstrap, cancelJob, compileFlowPreview, confirmFlow, createDiagnosticBundle, doctorCliProviders, doctorLocalModel, erasePrivateData, explainAnalysis, generateFlow, getJobSnapshot, getMaintenanceStatus, installStagedUpdate, listJobs, openReport, prepareManagedTools, previewGenerationContext, probeFlow, refreshDevices, repairFlow, resumeJob, runAndroid, runDemo, runIos, stageUpdate, trialGeneratedFlow } from "./api";
import type { CliProviderStatus, LocalModelStatus, MaintenanceStatus, StagedUpdate } from "./api";
import { DiagnosticCenter } from "./DiagnosticCenter";
import { FlowExplorer } from "./FlowExplorer";
import type {
  Bootstrap,
  AnalysisExplanation,
  CompiledFlow,
  Flow,
  FlowLock,
  FlowStep,
  GeneratedFlow,
  Job,
  JobAnalysis,
  JobPage,
  JobSnapshot,
  JobState,
  NormalizedResult,
  Platform,
  RedactedUiContext,
  TrialPreparation,
} from "./types";

type Stage = "compose" | "generated" | "locked" | "results";
type Page = "flow" | "explorer" | "devices" | "history" | "analysis" | "diagnostics" | "settings";
type Framework = "react-native" | "flutter" | "lynx";
type ProviderMode = "offline" | "local" | "codex" | "claude" | "cloud";
type FlowView = "steps" | "json" | "maestro";

interface PersistedFlowDraft {
  version: 1;
  intent: string;
  appId: string;
  framework: Framework;
  platform: Platform;
  providerMode: ProviderMode;
  generated?: GeneratedFlow;
  compiledFlow?: CompiledFlow;
  preparation?: TrialPreparation;
  flowLock?: FlowLock;
  runPreset: "quick" | "standard";
}

const FLOW_DRAFT_KEY = "reactor.flow-draft.v1";

const frameworkNames: Record<string, string> = {
  "react-native": "React Native",
  flutter: "Flutter",
  lynx: "Lynx",
};

const providerNames: Record<ProviderMode, string> = {
  offline: "Reactor Offline",
  local: "Local Model",
  codex: "Codex CLI",
  claude: "Claude Code CLI",
  cloud: "Cloud AI",
};

const stepNames: Record<FlowStep["action"], string> = {
  reset_app_state: "清理应用状态",
  launch_app: "启动应用",
  tap: "点击控件",
  input_text: "输入文本",
  swipe: "滑动页面",
  wait_for: "等待界面",
  assert_visible: "验证控件",
  pause: "等待",
  repeat: "重复操作",
};

const jobStateNames: Record<JobState, string> = {
  queued: "等待启动",
  preflight: "环境预检",
  warmup: "非计分预热",
  measuring: "正式测量",
  normalizing: "整理证据",
  completed: "已完成",
  failed: "已失败",
  cancelled: "已取消",
};

function App() {
  const [theme, setTheme] = useState<"dark" | "light">("dark");
  const [page, setPage] = useState<Page>("flow");
  const [environment, setEnvironment] = useState<Bootstrap>();
  const [stage, setStage] = useState<Stage>("compose");
  const [intent, setIntent] = useState("启动应用，进入列表页面，向上滚动 10 次并测量滚动性能");
  const [appId, setAppId] = useState("com.reactor.bench.reactnative");
  const [framework, setFramework] = useState<Framework>("react-native");
  const [platform, setPlatform] = useState<Platform>("android");
  const [selectedDeviceId, setSelectedDeviceId] = useState("");
  const [providerOpen, setProviderOpen] = useState(false);
  const [providerMode, setProviderMode] = useState<ProviderMode>("offline");
  const [apiKey, setApiKey] = useState("");
  const [saveApiKey, setSaveApiKey] = useState(false);
  const [useSavedApiKey, setUseSavedApiKey] = useState(false);
  const [endpoint, setEndpoint] = useState("https://api.openai.com/v1");
  const [model, setModel] = useState("gpt-5-mini");
  const [localEndpoint, setLocalEndpoint] = useState("http://127.0.0.1:11434");
  const [localModel, setLocalModel] = useState("qwen2.5:7b");
  const [localModelStatus, setLocalModelStatus] = useState<LocalModelStatus>();
  const [checkingLocalModel, setCheckingLocalModel] = useState(false);
  const [cliModel, setCliModel] = useState("");
  const [codexExecutable, setCodexExecutable] = useState("");
  const [claudeExecutable, setClaudeExecutable] = useState("");
  const [cliProviders, setCliProviders] = useState<CliProviderStatus[]>([]);
  const [checkingCli, setCheckingCli] = useState(false);
  const [generationContext, setGenerationContext] = useState<RedactedUiContext>();
  const [includeGenerationContext, setIncludeGenerationContext] = useState(false);
  const [readingGenerationContext, setReadingGenerationContext] = useState(false);
  const [generated, setGenerated] = useState<GeneratedFlow>();
  const [compiledFlow, setCompiledFlow] = useState<CompiledFlow>();
  const [flowView, setFlowView] = useState<FlowView>("steps");
  const [flowCopied, setFlowCopied] = useState(false);
  const [flowEditing, setFlowEditing] = useState(false);
  const [flowJsonDraft, setFlowJsonDraft] = useState("");
  const [flowEditError, setFlowEditError] = useState("");
  const [flowEditNotice, setFlowEditNotice] = useState("");
  const [flowLock, setFlowLock] = useState<FlowLock>();
  const [preparation, setPreparation] = useState<TrialPreparation>();
  const [results, setResults] = useState<NormalizedResult[]>([]);
  const [reportPath, setReportPath] = useState("");
  const [activeJob, setActiveJob] = useState<JobSnapshot>();
  const [runPreset, setRunPreset] = useState<"quick" | "standard">("quick");
  const [cancelling, setCancelling] = useState(false);
  const [preparingTools, setPreparingTools] = useState(false);
  const [refreshingDevices, setRefreshingDevices] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [history, setHistory] = useState<JobPage>();
  const [historySelection, setHistorySelection] = useState<JobSnapshot>();
  const [historyLoading, setHistoryLoading] = useState(false);
  const [historyLoadingOlder, setHistoryLoadingOlder] = useState(false);
  const [draftHydrated, setDraftHydrated] = useState(false);
  const flowCardRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);

  useEffect(() => {
    if (generated && !flowEditing) setFlowJsonDraft(JSON.stringify(generated.flow, null, 2));
  }, [generated, flowEditing]);

  useEffect(() => {
    const draft = loadFlowDraft();
    if (draft) {
      setIntent(draft.intent);
      setAppId(draft.appId);
      setFramework(draft.framework);
      setPlatform(draft.platform);
      setProviderMode(draft.providerMode);
      setGenerated(draft.generated);
      setCompiledFlow(draft.compiledFlow);
      setPreparation(draft.preparation);
      setFlowLock(draft.flowLock);
      setRunPreset(draft.runPreset);
      setStage(draft.flowLock ? "locked" : draft.generated ? "generated" : "compose");
      if (draft.generated) {
        void compileFlowPreview(draft.generated.flow)
          .then(setCompiledFlow)
          .catch((reason) => {
            setCompiledFlow(undefined);
            setPreparation(undefined);
            setFlowLock(undefined);
            setStage("generated");
            setFlowEditing(true);
            setError(`已保存 Flow 不符合当前可信性规则，请补充导航和目标页验证后重新试跑：${String(reason)}`);
          });
      }
    }
    setDraftHydrated(true);
  }, []);

  useEffect(() => {
    if (!draftHydrated) return;
    const draft: PersistedFlowDraft = {
      version: 1,
      intent,
      appId,
      framework,
      platform,
      providerMode,
      generated,
      compiledFlow,
      preparation,
      flowLock,
      runPreset,
    };
    try {
      window.localStorage.setItem(FLOW_DRAFT_KEY, JSON.stringify(draft));
    } catch (reason) {
      setError(`保存 Flow 草稿失败：${String(reason)}`);
    }
  }, [draftHydrated, intent, appId, framework, platform, providerMode, generated, compiledFlow, preparation, flowLock, runPreset]);

  useEffect(() => {
    bootstrap()
      .then((nextEnvironment) => {
        setEnvironment(nextEnvironment);
        if (!nextEnvironment.activeJob) return;
        setBusy(true);
        void resumeJob(nextEnvironment.activeJob.id, setActiveJob)
          .then(applyOutput)
          .catch(handleRunFailure)
          .finally(() => setBusy(false));
      })
      .catch((reason) => setError(String(reason)));
    void refreshCliProviders();
    void refreshLocalModel();
  }, []);

  useEffect(() => {
    if (page === "history" && !history) void loadHistory(0);
  }, [page, history]);

  useEffect(() => {
    if (page !== "history" || !historySelection || isTerminal(historySelection.job.state)) return;
    let cancelled = false;
    let timer = 0;
    let cursor = historySelection.events.at(-1)?.id ?? 0;
    const jobId = historySelection.job.id;
    const poll = async () => {
      try {
        const update = await getJobSnapshot(jobId, { cursor, limit: 100 });
        if (cancelled) return;
        cursor = update.events.at(-1)?.id ?? cursor;
        setHistorySelection((current) => current?.job.id === jobId ? {
          ...update,
          events: mergeEvents(current.events, update.events),
          hasMoreEvents: current.hasMoreEvents,
        } : current);
        setHistory((current) => current ? {
          ...current,
          jobs: current.jobs.map((job) => job.id === jobId ? update.job : job),
        } : current);
        if (!isTerminal(update.job.state)) {
          timer = window.setTimeout(poll, JOB_POLL_INTERVAL_MS);
        }
      } catch (reason) {
        if (!cancelled) setError(String(reason));
      }
    };
    timer = window.setTimeout(poll, JOB_POLL_INTERVAL_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [page, historySelection?.job.id]);

  const pipeline = useMemo(
    () => [
      { id: "compose", label: "描述目标" },
      { id: "generated", label: "生成 Flow" },
      { id: "locked", label: "试跑锁定" },
      { id: "results", label: "执行分析" },
    ],
    [],
  );
  const availableTargets = useMemo(
    () => environment?.devices.filter((device) => device.platform === platform) ?? [],
    [environment, platform],
  );
  const selectedTarget = availableTargets.find((device) => device.id === selectedDeviceId) ?? availableTargets[0];
  const selectedCliStatus = (providerMode === "codex" || providerMode === "claude")
    ? cliProviders.find((provider) => provider.kind === (providerMode === "codex" ? "codex" : "claude-code"))
    : undefined;
  const providerBlocked = (providerMode === "cloud" && !apiKey && !useSavedApiKey)
    || (providerMode === "local" && (!localModelStatus?.available || !localModel.trim()))
    || ((providerMode === "codex" || providerMode === "claude") && (!selectedCliStatus?.available || !selectedCliStatus.authenticated));

  useEffect(() => {
    if (!selectedTarget) {
      setSelectedDeviceId("");
    } else if (selectedTarget.id !== selectedDeviceId) {
      setSelectedDeviceId(selectedTarget.id);
    }
  }, [selectedDeviceId, selectedTarget]);

  useEffect(() => {
    setGenerationContext(undefined);
    setIncludeGenerationContext(false);
  }, [appId, platform, selectedDeviceId]);
  const stageIndex = pipeline.findIndex((item) => item.id === stage);

  async function refreshCliProviders() {
    setCheckingCli(true);
    try {
      const providers = await doctorCliProviders({
        codexExecutable: codexExecutable || undefined,
        claudeExecutable: claudeExecutable || undefined,
      });
      setCliProviders(providers);
      const codex = providers.find((provider) => provider.kind === "codex");
      const claude = providers.find((provider) => provider.kind === "claude-code");
      if (!codexExecutable && codex?.executable) setCodexExecutable(codex.executable);
      if (!claudeExecutable && claude?.executable) setClaudeExecutable(claude.executable);
    } catch (reason) {
      setError(`检测本机 AI 工具失败：${String(reason)}`);
    } finally {
      setCheckingCli(false);
    }
  }

  async function refreshLocalModel() {
    setCheckingLocalModel(true);
    try {
      const status = await doctorLocalModel(localEndpoint);
      setLocalModelStatus(status);
      if (status.models.length > 0 && !status.models.includes(localModel)) setLocalModel(status.models[0]);
    } catch (reason) {
      setLocalModelStatus({ available: false, endpoint: localEndpoint, models: [], detail: String(reason) });
    } finally {
      setCheckingLocalModel(false);
    }
  }

  async function onGenerate() {
    setBusy(true);
    setError("");
    setActiveJob(undefined);
    try {
      const output = await generateFlow({
        intent,
        appId,
        platform,
        uiTree: includeGenerationContext ? generationContext?.uiTree : undefined,
        apiKey: providerMode === "cloud" ? apiKey || undefined : undefined,
        saveApiKey: providerMode === "cloud" && saveApiKey,
        useSavedApiKey: providerMode === "cloud" && useSavedApiKey,
        endpoint: providerMode === "cloud" ? endpoint : providerMode === "local" ? localEndpoint : undefined,
        model: providerMode === "cloud" ? model : providerMode === "local" ? localModel : providerMode === "codex" || providerMode === "claude" ? cliModel || undefined : undefined,
        provider: providerMode,
        cliExecutable: providerMode === "codex" ? codexExecutable || undefined : providerMode === "claude" ? claudeExecutable || undefined : undefined,
      });
      const compiled = await compileFlowPreview(output.flow);
      setGenerated(output);
      setCompiledFlow(compiled);
      setFlowView("steps");
      setFlowCopied(false);
      setFlowEditing(false);
      setFlowEditError("");
      setFlowEditNotice("Flow 已生成，可查看步骤、编辑 JSON 或预览 Maestro YAML。");
      setProviderOpen(false);
      if (saveApiKey && apiKey) {
        setApiKey("");
        setUseSavedApiKey(true);
      }
      setFlowLock(undefined);
      setPreparation(undefined);
      setResults([]);
      setReportPath("");
      setStage("generated");
      window.setTimeout(() => flowCardRef.current?.scrollIntoView({ behavior: "smooth", block: "start" }), 80);
    } catch (reason) {
      handleRunFailure(reason);
    } finally {
      setBusy(false);
    }
  }

  async function onPreviewGenerationContext() {
    if (!selectedTarget) {
      setError(`未检测到 ${platform === "ios" ? "iOS Simulator" : "Android Emulator/设备"}，无法读取目标界面。`);
      return;
    }
    setReadingGenerationContext(true);
    setError("");
    try {
      const context = await previewGenerationContext({
        appId,
        platform,
        deviceId: selectedTarget.id,
      });
      setGenerationContext(context);
      setIncludeGenerationContext(false);
    } catch (reason) {
      setGenerationContext(undefined);
      setIncludeGenerationContext(false);
      setError(`读取目标界面失败：${String(reason)}`);
    } finally {
      setReadingGenerationContext(false);
    }
  }

  async function onExploreFlow() {
    if (!selectedTarget || !generationContext || !includeGenerationContext || providerMode === "offline") return;
    setBusy(true);
    setError("");
    setActiveJob(undefined);
    try {
      const providerInput = {
        intent,
        appId,
        platform,
        uiTree: generationContext.uiTree,
        apiKey: providerMode === "cloud" ? apiKey || undefined : undefined,
        saveApiKey: providerMode === "cloud" && saveApiKey,
        useSavedApiKey: providerMode === "cloud" && useSavedApiKey,
        endpoint: providerMode === "cloud" ? endpoint : providerMode === "local" ? localEndpoint : undefined,
        model: providerMode === "cloud" ? model : providerMode === "local" ? localModel : cliModel || undefined,
        provider: providerMode,
        cliExecutable: providerMode === "codex" ? codexExecutable || undefined : providerMode === "claude" ? claudeExecutable || undefined : undefined,
      } as const;
      const probe = await probeFlow(providerInput);
      let probePreparation = await trialGeneratedFlow(probe, selectedTarget.id, generationContext);
      if (probePreparation.failure) {
        probePreparation = await repairFlow({
          preparation: probePreparation,
          deviceId: selectedTarget.id,
          endpoint: providerMode === "local" ? localEndpoint : endpoint,
          apiKey: apiKey || undefined,
          saveApiKey,
          useSavedApiKey,
          allowModelContext: true,
          provider: providerMode,
          cliExecutable: providerMode === "codex" ? codexExecutable || undefined : providerMode === "claude" ? claudeExecutable || undefined : undefined,
          model: providerMode === "local" ? localModel : providerMode === "codex" || providerMode === "claude" ? cliModel : model,
        });
      }
      if (probePreparation.failure || !probePreparation.trial || !probePreparation.context) {
        throw new Error(probePreparation.failure?.message ?? "探索入口执行后未能读取新页面");
      }
      const exploredTree = [
        "=== SOURCE SCREEN (before safe entry action) ===",
        generationContext.uiTree,
        "=== DESTINATION SCREEN (observed after real probe) ===",
        probePreparation.context.uiTree,
      ].join("\n");
      let nextGenerated = await generateFlow({ ...providerInput, uiTree: exploredTree });
      nextGenerated = {
        ...nextGenerated,
        notes: [
          ...nextGenerated.notes,
          `逐步探索已真实执行入口 Flow ${probe.flow.id}；起始页 ${generationContext.preview.elementCount} 个元素 → 新页面 ${probePreparation.context.preview.elementCount} 个元素`,
        ],
      };
      let nextCompiled = await compileFlowPreview(nextGenerated.flow);
      let nextPreparation = await trialGeneratedFlow(nextGenerated, selectedTarget.id, generationContext);
      if (nextPreparation.failure) {
        nextPreparation = await repairFlow({
          preparation: nextPreparation,
          deviceId: selectedTarget.id,
          endpoint: providerMode === "local" ? localEndpoint : endpoint,
          apiKey: apiKey || undefined,
          saveApiKey,
          useSavedApiKey,
          allowModelContext: true,
          provider: providerMode,
          cliExecutable: providerMode === "codex" ? codexExecutable || undefined : providerMode === "claude" ? claudeExecutable || undefined : undefined,
          model: providerMode === "local" ? localModel : providerMode === "codex" || providerMode === "claude" ? cliModel : model,
        });
        nextGenerated = nextPreparation.generated;
        nextCompiled = await compileFlowPreview(nextGenerated.flow);
      }
      setGenerated(nextGenerated);
      setCompiledFlow(nextCompiled);
      setPreparation(nextPreparation);
      setFlowLock(undefined);
      setResults([]);
      setReportPath("");
      setFlowView("steps");
      setFlowCopied(false);
      setFlowEditing(false);
      setFlowEditError("");
      setFlowEditNotice(nextPreparation.trial
        ? `探索试跑已通过；${nextPreparation.modelCalls} 次修复调用，等待人工检查并锁定。`
        : "探索尚未到达目标页，请检查失败证据后继续修复。"
      );
      setProviderOpen(false);
      setStage("generated");
      window.setTimeout(() => flowCardRef.current?.scrollIntoView({ behavior: "smooth", block: "start" }), 80);
      if (saveApiKey && apiKey) {
        setApiKey("");
        setUseSavedApiKey(true);
      }
    } catch (reason) {
      handleRunFailure(reason);
    } finally {
      setBusy(false);
    }
  }

  async function onTrial() {
    if (!generated) return;
    if (!selectedTarget) {
      setError(`未检测到 ${platform === "ios" ? "iOS Simulator" : "Android Emulator/设备"}。请先准备 Reactor 内置工具并启动目标；静态校验不能替代上机试跑。`);
      return;
    }
    setBusy(true);
    setError("");
    try {
      setPreparation(await trialGeneratedFlow(generated, selectedTarget?.id, generationContext));
    } catch (reason) {
      handleRunFailure(reason);
    } finally {
      setBusy(false);
    }
  }

  async function onRepair() {
    if (!preparation?.failure || !selectedTarget) return;
    setBusy(true);
    setError("");
    try {
      const repaired = await repairFlow({
        preparation,
        deviceId: selectedTarget.id,
        endpoint: providerMode === "local" ? localEndpoint : endpoint,
        apiKey: apiKey || undefined,
        saveApiKey,
        useSavedApiKey,
        allowModelContext: true,
        provider: providerMode,
        cliExecutable: providerMode === "codex" ? codexExecutable || undefined : providerMode === "claude" ? claudeExecutable || undefined : undefined,
        model: providerMode === "local" ? localModel : providerMode === "codex" || providerMode === "claude" ? cliModel : model,
      });
      setPreparation(repaired);
      setGenerated(repaired.generated);
      setCompiledFlow(await compileFlowPreview(repaired.generated.flow));
      setFlowCopied(false);
      setFlowEditing(false);
      setFlowEditNotice("AI 修复后的 Flow 已更新，请查看修改并重新确认。");
      window.setTimeout(() => flowCardRef.current?.scrollIntoView({ behavior: "smooth", block: "start" }), 80);
      if (saveApiKey && apiKey) {
        setApiKey("");
        setUseSavedApiKey(true);
      }
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function onConfirmFlow() {
    if (!preparation?.trial || preparation.failure) return;
    setBusy(true);
    setError("");
    try {
      setFlowLock(await confirmFlow(preparation));
      setStage("locked");
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function onDemo() {
    if (!flowLock) return;
    setBusy(true);
    setError("");
    setResults([]);
    setReportPath("");
    setActiveJob(undefined);
    try {
      const output = await runDemo(flowLock, setActiveJob);
      applyOutput(output);
    } catch (reason) {
      handleRunFailure(reason);
    } finally {
      setBusy(false);
    }
  }

  async function onRealRun() {
    if (!flowLock || !selectedTarget) return;
    if (flowLock.flow.appId !== appId || flowLock.flow.platform !== platform) {
      invalidateGeneratedFlow();
      setError("应用包名或平台已改变，请重新生成并试跑 Flow；Reactor 不会用旧锁定文件测量新输入。");
      return;
    }
    setBusy(true);
    setError("");
    setResults([]);
    setReportPath("");
    setActiveJob(undefined);
    try {
      const run = platform === "ios" ? runIos : runAndroid;
      const output = await run({
        flowLock,
        framework,
        scenario: generated?.flow.id.split("-")[0] ?? "custom",
        deviceId: selectedTarget.id,
        durationMs: runPreset === "standard" ? 18_000 : 5_000,
        iterations: runPreset === "standard" ? 10 : 1,
      }, setActiveJob);
      applyOutput(output);
    } catch (reason) {
      handleRunFailure(reason);
    } finally {
      setBusy(false);
    }
  }

  async function onRefresh() {
    setRefreshingDevices(true);
    setError("");
    try {
      setEnvironment(await refreshDevices());
    } catch (reason) {
      setError(String(reason));
    } finally {
      setRefreshingDevices(false);
    }
  }

  function useDeviceInFlow(deviceId: string, devicePlatform: string) {
    setPlatform(devicePlatform === "ios" ? "ios" : "android");
    setSelectedDeviceId(deviceId);
    navigateTo("flow");
  }

  function navigateTo(nextPage: Page) {
    setError("");
    setPage(nextPage);
  }

  async function onPrepareTools() {
    setPreparingTools(true);
    setError("");
    try {
      setEnvironment(await prepareManagedTools());
    } catch (reason) {
      setError(String(reason));
    } finally {
      setPreparingTools(false);
    }
  }

  async function onCancel() {
    if (!activeJob || ["completed", "failed", "cancelled"].includes(activeJob.job.state)) return;
    setCancelling(true);
    try {
      const job = await cancelJob(activeJob.job.id);
      setActiveJob((snapshot) => snapshot ? { ...snapshot, job } : snapshot);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setCancelling(false);
    }
  }

  async function loadHistory(offset: number) {
    setHistoryLoading(true);
    setError("");
    try {
      const next = await listJobs(20, offset);
      setHistory(next);
      const selectedId = historySelection?.job.id;
      const selected = next.jobs.find((job) => job.id === selectedId) ?? next.jobs[0];
      setHistorySelection(selected ? await getJobSnapshot(selected.id) : undefined);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setHistoryLoading(false);
    }
  }

  async function selectHistoryJob(job: Job) {
    setHistoryLoading(true);
    setError("");
    try {
      setHistorySelection(await getJobSnapshot(job.id));
    } catch (reason) {
      setError(String(reason));
    } finally {
      setHistoryLoading(false);
    }
  }

  async function loadOlderHistoryEvents() {
    const current = historySelection;
    const before = current?.events[0]?.id;
    if (!current || before === undefined || !current.hasMoreEvents) return;
    setHistoryLoadingOlder(true);
    setError("");
    try {
      const previous = await getJobSnapshot(current.job.id, { before, limit: 100 });
      setHistorySelection((latest) => latest?.job.id === current.job.id ? {
        ...latest,
        events: mergeEvents(previous.events, latest.events),
        hasMoreEvents: previous.hasMoreEvents,
      } : latest);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setHistoryLoadingOlder(false);
    }
  }

  function applyOutput(output: { results: NormalizedResult[]; reportPath?: string }) {
    setResults(output.results);
    setReportPath(output.reportPath ?? "");
    setStage("results");
  }

  function handleRunFailure(reason: unknown) {
    const message = String(reason);
    if (message.includes("cancelled by user") || message.includes("任务已取消")) {
      setError("");
      return;
    }
    setError(message);
  }

  function invalidateGeneratedFlow() {
    setGenerated(undefined);
    setCompiledFlow(undefined);
    setFlowView("steps");
    setFlowCopied(false);
    setFlowEditing(false);
    setFlowJsonDraft("");
    setFlowEditError("");
    setFlowEditNotice("");
    setFlowLock(undefined);
    setPreparation(undefined);
    setResults([]);
    setReportPath("");
    setStage("compose");
  }

  function beginFlowEdit() {
    if (!generated) return;
    setFlowView("json");
    setFlowJsonDraft(JSON.stringify(generated.flow, null, 2));
    setFlowEditError("");
    setFlowEditing(true);
  }

  function cancelFlowEdit() {
    if (generated) setFlowJsonDraft(JSON.stringify(generated.flow, null, 2));
    setFlowEditError("");
    setFlowEditing(false);
  }

  async function applyFlowEdit() {
    if (!generated) return;
    setBusy(true);
    setFlowEditError("");
    try {
      const flow = JSON.parse(flowJsonDraft) as Flow;
      const compiled = await compileFlowPreview(flow);
      setGenerated({
        ...generated,
        flow,
        notes: [...generated.notes, "Flow JSON manually edited and revalidated"],
      });
      setCompiledFlow(compiled);
      setPreparation(undefined);
      setFlowLock(undefined);
      setResults([]);
      setReportPath("");
      setStage("generated");
      setFlowEditing(false);
      setFlowView("steps");
      setFlowEditNotice("修改已通过 Rust 校验，Maestro YAML 已重新编译；旧试跑与锁定已失效。");
      window.setTimeout(() => flowCardRef.current?.scrollIntoView({ behavior: "smooth", block: "start" }), 80);
    } catch (reason) {
      setFlowEditError(`无法应用修改：${String(reason)}`);
    } finally {
      setBusy(false);
    }
  }

  async function copyFlowSource() {
    if (!generated) return;
    const source = flowView === "json"
      ? JSON.stringify(generated.flow, null, 2)
      : maestroSource(compiledFlow);
    try {
      await navigator.clipboard.writeText(source);
      setFlowCopied(true);
      window.setTimeout(() => setFlowCopied(false), 1600);
    } catch (reason) {
      setError(`复制 Flow 失败：${String(reason)}`);
    }
  }

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark"><Zap size={20} strokeWidth={2.4} /></div>
          <div><strong>Reactor</strong><span>Performance Lab</span></div>
        </div>
        <nav>
          <button className={`nav-item ${page === "flow" ? "active" : ""}`} onClick={() => navigateTo("flow")}><WandSparkles size={18} />Flow Studio</button>
          <button className={`nav-item ${page === "explorer" ? "active" : ""}`} onClick={() => navigateTo("explorer")}><ScanSearch size={18} />Flow Explorer<span className="nav-beta">M8.10</span></button>
          <button className={`nav-item ${page === "devices" ? "active" : ""}`} onClick={() => navigateTo("devices")}><Smartphone size={18} />设备实验室<span className="nav-count">{environment?.devices.length ?? 0}</span></button>
          <button className={`nav-item ${page === "history" ? "active" : ""}`} onClick={() => navigateTo("history")}><Activity size={18} />运行记录</button>
          <button className={`nav-item ${page === "analysis" ? "active" : ""}`} onClick={() => navigateTo("analysis")}><CircleGauge size={18} />结果分析</button>
          <button className={`nav-item ${page === "diagnostics" ? "active" : ""}`} onClick={() => navigateTo("diagnostics")}><Flame size={18} />性能诊断</button>
        </nav>
        <div className="sidebar-bottom">
          <button className={`nav-item ${page === "settings" ? "active" : ""}`} onClick={() => navigateTo("settings")}><Settings2 size={18} />设置</button>
          <div className="privacy-note"><ShieldCheck size={16} /><span>AI 不进入测量窗口</span></div>
        </div>
      </aside>

      <main className="workspace">
        {page === "explorer" ? (
          <FlowExplorer
            devices={environment?.devices ?? []}
            selectedDeviceId={selectedDeviceId}
            appId={appId}
            goal={intent}
            ai={{
              provider: providerMode,
              endpoint: providerMode === "local" ? localEndpoint : endpoint,
              model: providerMode === "local" ? localModel : providerMode === "codex" || providerMode === "claude" ? cliModel : model,
              apiKey: providerMode === "cloud" ? apiKey || undefined : undefined,
              saveApiKey: providerMode === "cloud" && saveApiKey,
              useSavedApiKey: providerMode === "cloud" && useSavedApiKey,
              cliExecutable: providerMode === "codex" ? codexExecutable || undefined : providerMode === "claude" ? claudeExecutable || undefined : undefined,
            }}
            activeJobRunning={Boolean(activeJob && !["completed", "failed", "cancelled"].includes(activeJob.job.state))}
            onSelectDevice={(device) => {
              setSelectedDeviceId(device.id);
              setPlatform(device.platform === "ios" ? "ios" : "android");
            }}
            onAppIdChange={(value) => {
              setAppId(value);
              invalidateGeneratedFlow();
            }}
            onRefreshDevices={() => void onRefresh()}
            onPerformanceHandoff={(lock, nextPreparation, compiled) => {
              setGenerated(nextPreparation.generated);
              setCompiledFlow(compiled);
              setPreparation(nextPreparation);
              setFlowLock(lock);
              setStage("locked");
              setResults([]);
              setReportPath("");
              setFlowEditNotice("Flow Explorer 已完成真实回放、目标页唯一性证明和哈希锁定；请选择采集预设后开始正式测量。");
              setPage("flow");
            }}
          />
        ) : page === "devices" ? (
          <DeviceLab
            environment={environment}
            selectedDeviceId={selectedDeviceId}
            refreshing={refreshingDevices}
            preparingTools={preparingTools}
            onRefresh={onRefresh}
            onPrepareTools={onPrepareTools}
            onUseDevice={useDeviceInFlow}
          />
        ) : page === "history" ? (
          <HistoryCenter
            history={history}
            selected={historySelection}
            loading={historyLoading}
            error={error}
            onRefresh={() => loadHistory(history?.offset ?? 0)}
            onPage={loadHistory}
            onSelect={selectHistoryJob}
            loadingOlder={historyLoadingOlder}
            onLoadOlder={loadOlderHistoryEvents}
          />
        ) : page === "analysis" ? (
          <AnalysisCenter />
        ) : page === "diagnostics" ? (
          <DiagnosticCenter onNavigate={navigateTo} />
        ) : page === "settings" ? (
          <SettingsCenter
            environment={environment}
            cliProviders={cliProviders}
            localModelStatus={localModelStatus}
            checkingCli={checkingCli}
            checkingLocalModel={checkingLocalModel}
            preparingTools={preparingTools}
            onRefreshCli={refreshCliProviders}
            onRefreshLocal={refreshLocalModel}
            onPrepareTools={onPrepareTools}
          />
        ) : (
        <>
        <header className="topbar">
          <div>
            <p className="eyebrow">FLOW STUDIO</p>
            <h1>把测试目标交给 Reactor</h1>
          </div>
          <div className="top-actions">
            <span className={`status-pill ${environment?.doctor.ready ? "ready" : "waiting"}`}>
              <span className="status-dot" />{environment === undefined ? "正在检查环境" : environment.doctor.ready ? "工具链已就绪" : "内置工具待准备"}
            </span>
            <button className="icon-button" onClick={() => setTheme(theme === "dark" ? "light" : "dark")} aria-label="切换主题">
              {theme === "dark" ? <Sun size={18} /> : <Moon size={18} />}
            </button>
          </div>
        </header>

        <section className="pipeline" aria-label="Flow 进度">
          {pipeline.map((item, index) => (
            <div className={`pipeline-step ${index <= stageIndex ? "done" : ""}`} key={item.id}>
              <span>{index < stageIndex ? <Check size={13} /> : index + 1}</span>
              <b>{item.label}</b>
              {index < pipeline.length - 1 && <ArrowRight size={14} className="pipeline-arrow" />}
            </div>
          ))}
        </section>

        {error && <div className="error-banner">{error}</div>}

        {activeJob && <RunStatus snapshot={activeJob} cancelling={cancelling} onCancel={onCancel} />}

        <div className="content-grid">
          <section className="primary-column">
            <div className="composer-card card">
              <div className="card-heading">
                <div className="heading-icon purple"><Bot size={19} /></div>
                <div><h2>测试指令</h2><p>描述用户路径和你关心的性能目标</p></div>
                <span className="local-badge">{providerMode === "offline" ? "Reactor Offline · 非大模型" : providerNames[providerMode]}</span>
              </div>
              <textarea value={intent} disabled={busy} onChange={(event) => { setIntent(event.target.value); invalidateGeneratedFlow(); }} placeholder="例如：启动应用，进入商品列表，连续滚动并打开详情页……" />
              <div className="composer-options">
                <label><span>应用包名</span><input value={appId} disabled={busy} onChange={(event) => { setAppId(event.target.value); invalidateGeneratedFlow(); }} /></label>
                <div className="platform-field">
                  <span>应用框架</span>
                  <div className="framework-selector" role="group" aria-label="应用框架">
                    <button type="button" disabled={busy} aria-pressed={framework === "react-native"} className={framework === "react-native" ? "active" : ""} onClick={() => { setFramework("react-native"); invalidateGeneratedFlow(); }}>RN</button>
                    <button type="button" disabled={busy} aria-pressed={framework === "flutter"} className={framework === "flutter" ? "active" : ""} onClick={() => { setFramework("flutter"); invalidateGeneratedFlow(); }}>Flutter</button>
                    <button type="button" disabled={busy} aria-pressed={framework === "lynx"} className={framework === "lynx" ? "active" : ""} onClick={() => { setFramework("lynx"); invalidateGeneratedFlow(); }}>Lynx</button>
                  </div>
                </div>
                <div className="platform-field">
                  <span>平台</span>
                  <div className="platform-selector" role="group" aria-label="平台">
                    <button type="button" disabled={busy} aria-pressed={platform === "android"} className={platform === "android" ? "active" : ""} onClick={() => { setPlatform("android"); invalidateGeneratedFlow(); }}>Android</button>
                    <button type="button" disabled={busy} aria-pressed={platform === "ios"} className={platform === "ios" ? "active" : ""} onClick={() => { setPlatform("ios"); invalidateGeneratedFlow(); }}>iOS</button>
                  </div>
                </div>
              </div>
              {providerMode !== "offline" && (
                <div className="generation-context-card">
                  <div className="generation-context-heading">
                    <div>
                      <b>目标界面上下文</b>
                      <span>只读取当前目标的无障碍文本并在本机脱敏；截图不会发送给模型。</span>
                    </div>
                    <button type="button" className="secondary-button compact" disabled={busy || readingGenerationContext || !selectedTarget || !appId.trim()} onClick={() => void onPreviewGenerationContext()}>
                      {readingGenerationContext ? <RefreshCw size={14} className="spin" /> : <Smartphone size={14} />}{readingGenerationContext ? "读取中…" : generationContext ? "重新读取" : "读取当前界面"}
                    </button>
                  </div>
                  {generationContext ? (
                    <>
                      <div className="generation-context-summary">
                        <span>{generationContext.preview.elementCount} 个元素</span>
                        <span>{generationContext.preview.includedChars} 字符</span>
                        <span>{generationContext.preview.redactionCount} 处脱敏</span>
                        <span>0 字节截图上传</span>
                      </div>
                      <label className="credential-option generation-context-consent">
                        <input type="checkbox" checked={includeGenerationContext} onChange={(event) => setIncludeGenerationContext(event.target.checked)} />
                        <span>仅本次生成/探索允许将上述脱敏文本提供给 {providerNames[providerMode]}</span>
                      </label>
                      <details className="generation-context-preview">
                        <summary>查看脱敏内容</summary>
                        <pre>{generationContext.uiTree}</pre>
                      </details>
                    </>
                  ) : (
                    <p>未提供界面上下文时，AI 可能不知道“列表页”等入口的真实控件名称；试跑只能验证命令是否执行。</p>
                  )}
                </div>
              )}
              <div className="provider-mode provider-mode-select" role="group" aria-label="选择 Flow 生成器">
                <button type="button" className={providerMode === "offline" ? "active" : ""} aria-pressed={providerMode === "offline"} onClick={() => { setProviderMode("offline"); invalidateGeneratedFlow(); }}>Reactor Offline</button>
                <button type="button" className={providerMode === "local" ? "active" : ""} aria-pressed={providerMode === "local"} onClick={() => { setProviderMode("local"); invalidateGeneratedFlow(); }}>Local Model</button>
                <button type="button" className={providerMode === "codex" ? "active" : ""} aria-pressed={providerMode === "codex"} onClick={() => { setProviderMode("codex"); invalidateGeneratedFlow(); }}>Codex CLI</button>
                <button type="button" className={providerMode === "claude" ? "active" : ""} aria-pressed={providerMode === "claude"} onClick={() => { setProviderMode("claude"); invalidateGeneratedFlow(); }}>Claude Code</button>
                <button type="button" className={providerMode === "cloud" ? "active" : ""} aria-pressed={providerMode === "cloud"} onClick={() => { setProviderMode("cloud"); invalidateGeneratedFlow(); }}>Cloud AI</button>
              </div>
              <button className="provider-toggle" onClick={() => setProviderOpen(!providerOpen)}>
                <Sparkles size={15} />配置 {providerNames[providerMode]}（{providerMode === "cloud" ? "需要 Key" : "无需 Key"}）<ChevronDown size={15} className={providerOpen ? "rotated" : ""} />
              </button>
              {providerOpen && (
                <div className="provider-panel">
                  {providerMode === "offline" ? (
                    <div className="provider-offline wide"><ShieldCheck size={16} /><div><b>内置规则生成器</b><span>不调用模型、不需要 API Key；适合零配置体验和确定性模板。复杂自然语言请选择 Cloud AI。</span></div></div>
                  ) : providerMode === "local" ? (
                    <>
                      <div className={`provider-offline wide ${localModelStatus?.available ? "ready" : "warning"}`}>
                        <Cpu size={16} />
                        <div><b>本地模型 · {checkingLocalModel ? "检测中…" : localModelStatus?.available ? "已连接" : "未连接"}</b><span>{localModelStatus?.detail ?? "支持 Ollama、LM Studio 和其他 OpenAI-compatible 本地服务；提示词和 UI 上下文不离开本机。"}</span></div>
                        <button type="button" className="secondary-button compact" disabled={checkingLocalModel} onClick={() => void refreshLocalModel()}>{checkingLocalModel ? "检测中" : "重新检测"}</button>
                      </div>
                      <label className="wide"><span>本地服务地址</span><input value={localEndpoint} onChange={(event) => { setLocalEndpoint(event.target.value); setLocalModelStatus(undefined); }} placeholder="http://127.0.0.1:11434" /></label>
                      <label className="wide"><span>Model</span>{localModelStatus?.models.length ? <select value={localModel} onChange={(event) => setLocalModel(event.target.value)}>{localModelStatus.models.map((name) => <option value={name} key={name}>{name}</option>)}</select> : <input value={localModel} onChange={(event) => setLocalModel(event.target.value)} placeholder="例如 qwen2.5:7b" />}</label>
                      <div className="provider-offline wide"><LockKeyhole size={16} /><div><b>无需 API Key，本机处理</b><span>Reactor 只在 Flow 生成/修复阶段访问该地址；正式性能测量期间不会调用本地模型。</span></div></div>
                    </>
                  ) : providerMode === "codex" || providerMode === "claude" ? (
                    <>
                      <div className={`provider-offline wide ${selectedCliStatus?.available && selectedCliStatus.authenticated ? "ready" : "warning"}`}>
                        <ShieldCheck size={16} />
                        <div><b>{providerNames[providerMode]} · {selectedCliStatus?.version ?? (checkingCli ? "检测中…" : "尚未检测")}</b><span>{selectedCliStatus?.detail ?? "复用本机已有安装与登录状态；Reactor 不读取凭据，也不会自动安装。"}</span></div>
                        <button type="button" className="secondary-button compact" disabled={checkingCli} onClick={() => void refreshCliProviders()}>{checkingCli ? "检测中" : "重新检测"}</button>
                      </div>
                      <label className="wide"><span>可执行文件路径</span><input value={providerMode === "codex" ? codexExecutable : claudeExecutable} onChange={(event) => providerMode === "codex" ? setCodexExecutable(event.target.value) : setClaudeExecutable(event.target.value)} placeholder={providerMode === "codex" ? "/Applications/Codex.app/Contents/Resources/codex" : "/opt/homebrew/bin/claude"} /></label>
                      <label className="wide"><span>Model（留空则使用 CLI 默认模型）</span><input value={cliModel} onChange={(event) => setCliModel(event.target.value)} placeholder="使用 CLI 默认模型" /></label>
                      <div className="provider-offline wide"><LockKeyhole size={16} /><div><b>只在 Flow 生成/修复阶段调用</b><span>禁用写入工具和权限交互，输出必须通过 Reactor Flow Schema；正式性能测量期间绝不调用模型。</span></div></div>
                    </>
                  ) : (
                    <>
                      <label><span>Base URL 或完整端点（自动兼容 Responses / Chat Completions）</span><input value={endpoint} onChange={(event) => setEndpoint(event.target.value)} placeholder="https://provider.example/v1" /></label>
                      <label><span>Model</span><input value={model} onChange={(event) => setModel(event.target.value)} /></label>
                      <label className="wide"><span>API Key（仅保留在当前会话）</span><input type="password" value={apiKey} onChange={(event) => setApiKey(event.target.value)} placeholder="sk-…" /></label>
                      <label className="credential-option"><input type="checkbox" checked={saveApiKey} onChange={(event) => setSaveApiKey(event.target.checked)} /><span>保存到系统钥匙串</span></label>
                      <label className="credential-option"><input type="checkbox" checked={useSavedApiKey} onChange={(event) => setUseSavedApiKey(event.target.checked)} /><span>使用已保存的密钥</span></label>
                    </>
                  )}
                </div>
              )}
              <div className="composer-footer">
                <p><ShieldCheck size={14} />生成发生在测量前；正式运行只执行锁定 Flow</p>
                <div className="composer-actions">
                  {providerMode !== "offline" && generationContext && includeGenerationContext && (
                    <button className="secondary-button" disabled={busy || !selectedTarget || providerBlocked || !intent.trim() || !appId.trim()} onClick={() => void onExploreFlow()} title="在准备阶段生成、真实试跑，并在失败时使用脱敏界面证据最多修复两次">
                      {busy ? <RefreshCw size={17} className="spin" /> : <Smartphone size={17} />}AI 探索并试跑
                    </button>
                  )}
                  <button className="primary-button" disabled={busy || !intent.trim() || !appId.trim() || providerBlocked} onClick={onGenerate}>
                    {busy ? <RefreshCw size={17} className="spin" /> : <WandSparkles size={17} />}{providerMode === "offline" ? "生成 Offline Flow" : `使用 ${providerNames[providerMode]} 生成`}
                  </button>
                </div>
              </div>
            </div>

            {generated && (
              <div className="flow-ready-banner" role="status">
                <Check size={18} />
                <div><b>Flow 已生成：{generated.flow.name}</b><span>{flowEditNotice || "可以查看完整步骤、Flow JSON 和 Maestro YAML，也可以在试跑前修改。"}</span></div>
                <button className="secondary-button" onClick={() => { beginFlowEdit(); window.setTimeout(() => flowCardRef.current?.scrollIntoView({ behavior: "smooth", block: "start" }), 40); }}><Pencil size={15} />查看并编辑 Flow</button>
              </div>
            )}

            {generated && (
              <div className="flow-card card" ref={flowCardRef}>
                <div className="card-heading">
                  <div className="heading-icon green"><FlaskConical size={19} /></div>
                  <div><h2>{generated.flow.name}</h2><p>{generated.provider === "reactor" ? "Reactor Offline（规则生成，非大模型）" : generated.provider} · {generated.model}</p></div>
                  <div className="flow-card-tools">
                    <span className="schema-badge">FLOW v{generated.flow.schemaVersion}</span>
                    <div className="flow-view-tabs" role="tablist" aria-label="Flow 内容视图">
                      <button role="tab" aria-selected={flowView === "steps"} className={flowView === "steps" ? "active" : ""} onClick={() => setFlowView("steps")}>步骤</button>
                      <button role="tab" aria-selected={flowView === "json"} className={flowView === "json" ? "active" : ""} onClick={() => setFlowView("json")}>Flow JSON</button>
                      <button role="tab" aria-selected={flowView === "maestro"} className={flowView === "maestro" ? "active" : ""} onClick={() => setFlowView("maestro")}>Maestro YAML</button>
                    </div>
                  </div>
                </div>
                {flowView === "steps" ? (
                  <>
                    <FlowTimeline title="测量前" steps={generated.flow.setup} muted />
                    <FlowTimeline title="正式测量窗口" steps={generated.flow.measured} />
                    {generated.flow.teardown.length > 0 && <FlowTimeline title="测量后" steps={generated.flow.teardown} muted />}
                  </>
                ) : flowView === "json" && flowEditing ? (
                  <div className="flow-editor">
                    <div className="flow-source-heading">
                      <div><b>编辑 Reactor Flow JSON</b><span>保存前由 Rust 校验并重新编译 Maestro YAML；修改会使旧试跑和锁定失效</span></div>
                      <div className="flow-source-actions">
                        <button className="secondary-button" disabled={busy} onClick={cancelFlowEdit}>取消</button>
                        <button className="primary-button" disabled={busy || !flowJsonDraft.trim()} onClick={() => void applyFlowEdit()}>{busy ? <RefreshCw size={15} className="spin" /> : <Save size={15} />}校验并应用</button>
                      </div>
                    </div>
                    <textarea aria-label="Reactor Flow JSON 编辑器" value={flowJsonDraft} onChange={(event) => { setFlowJsonDraft(event.target.value); setFlowEditError(""); }} spellCheck={false} />
                    {flowEditError && <div className="flow-editor-error">{flowEditError}</div>}
                  </div>
                ) : (
                  <FlowSource
                    kind={flowView}
                    source={flowView === "json" ? JSON.stringify(generated.flow, null, 2) : maestroSource(compiledFlow)}
                    copied={flowCopied}
                    onCopy={copyFlowSource}
                    onEdit={flowView === "json" ? beginFlowEdit : undefined}
                  />
                )}
                {preparation && <PreparationReview preparation={preparation} />}
                <div className="flow-actions">
                  {flowLock ? (
                    <div className="hash-block"><LockKeyhole size={16} /><span>已锁定</span><code>{flowLock.flow.appId} · {flowLock.flowHash.slice(0, 12)}…</code></div>
                  ) : <p>{availableTargets.length ? "先在模拟器/设备试跑，通过后生成不可变哈希。" : `先准备 Reactor 内置工具并启动 ${platform === "ios" ? "iOS Simulator" : "Android Emulator"}；静态校验不能替代上机试跑。`}</p>}
                  {!flowLock && !preparation ? (
                    <button className="secondary-button" disabled={busy || !selectedTarget} onClick={onTrial}><Play size={16} />{selectedTarget ? "在目标试跑" : "等待测试目标"}</button>
                  ) : !flowLock && preparation?.failure ? (
                    preparation.failure.code === "target_unavailable" ? <button className="secondary-button" disabled={busy} onClick={() => { setPreparation(undefined); void onRefresh(); }}><RefreshCw size={16} />准备/刷新测试目标</button> : <button className="primary-button" disabled={busy || !selectedTarget || providerMode === "offline" || (providerMode === "cloud" && !apiKey && !useSavedApiKey) || (providerMode === "local" && !localModelStatus?.available) || ((providerMode === "codex" || providerMode === "claude") && (!selectedCliStatus?.available || !selectedCliStatus.authenticated))} onClick={onRepair}><WandSparkles size={16} />AI 自愈（最多 2 次）</button>
                  ) : !flowLock && preparation?.trial ? (
                    preparation.trial.synthetic ? <button className="secondary-button" disabled={busy || !selectedTarget} onClick={() => setPreparation(undefined)}><Play size={16} />改用目标真实试跑</button> : <button className="primary-button" disabled={busy} onClick={onConfirmFlow}><LockKeyhole size={16} />{preparation.changes.length ? "确认修改并锁定" : "确认并锁定"}</button>
                  ) : flowLock ? (
                    <div className="run-buttons">
                      <label className="run-preset"><span>采集预设</span><select value={runPreset} onChange={(event) => setRunPreset(event.target.value as "quick" | "standard")}><option value="quick">快速验收 · 1 次 × 5 秒</option><option value="standard">正式基准 · 10 次 × 18 秒</option></select></label>
                      {flowLock.trial?.synthetic && availableTargets.length > 0 && (
                        <button className="secondary-button" disabled={busy} onClick={() => { setFlowLock(undefined); setPreparation(undefined); }}>改用模拟器试跑</button>
                      )}
                      <button className="secondary-button" disabled={busy} onClick={onDemo} title="使用明确标记的虚拟指标预览三框架报告布局，不会测量真实应用">三框架模拟导览</button>
                      <button className="primary-button" disabled={busy || !selectedTarget || flowLock.trial?.synthetic !== false} onClick={onRealRun} title={selectedTarget ? `在所选 ${platform === "ios" ? "iOS Simulator 使用 xctrace" : "Android 模拟器或设备"}执行测量` : `启动 ${platform === "ios" ? "iOS Simulator" : "Android Emulator"} 后启用`}>{busy ? <RefreshCw size={17} className="spin" /> : <Play size={17} />}{selectedTarget ? platform === "ios" ? "iOS xctrace 运行" : "模拟器/设备运行" : `等待 ${platform === "ios" ? "iOS Simulator" : "Android Emulator"}`}</button>
                    </div>
                  ) : null}
                </div>
              </div>
            )}

            {results.length > 0 && <Results results={results} reportPath={reportPath} />}
          </section>

          <aside className="inspector-column">
            <div className="card environment-card">
              <div className="mini-heading"><h3>运行环境</h3><button className="icon-button small" onClick={onRefresh}><RefreshCw size={14} /></button></div>
              <div className="environment-list">
                {environment?.doctor.checks.map((check) => (
                  <div className="environment-row" key={check.id}>
                    <div className={`tool-icon ${check.available ? "ok" : "missing"}`}>
                      {check.id === "adb" ? <Smartphone size={15} /> : check.id === "java" ? <Cpu size={15} /> : <HardDrive size={15} />}
                    </div>
                    <div><b>{check.label}</b><span>{check.available ? "Reactor 内置版本" : "等待准备"}</span></div>
                    {check.available && <Check size={15} className="check" />}
                  </div>
                )) ?? <div className="skeleton-list" />}
              </div>
              <div className="device-summary">
                <div><Smartphone size={18} /><span><b>{availableTargets.length}</b> 个 {platform === "ios" ? "iOS" : "Android"} 目标</span></div>
                {availableTargets.length > 1 ? (
                  <select className="device-select" value={selectedTarget?.id ?? ""} onChange={(event) => setSelectedDeviceId(event.target.value)}>
                    {availableTargets.map((device) => <option value={device.id} key={device.id}>{device.name ?? device.id} · {device.physical ? "物理设备" : "模拟器"}</option>)}
                  </select>
                ) : (
                  <span>{selectedTarget ? `${selectedTarget.name ?? selectedTarget.id} · ${selectedTarget.physical ? "物理设备" : "模拟器"} · ${selectedTarget.metadata.osVersion ?? "OS 未知"}` : `启动 ${platform === "ios" ? "iOS Simulator" : "Android Emulator"} 后可执行测量`}</span>
                )}
              </div>
              {!environment?.doctor.ready && (
                <button className="primary-button setup-button" disabled={preparingTools} onClick={onPrepareTools}>
                  {preparingTools ? <RefreshCw size={15} className="spin" /> : <HardDrive size={15} />}
                  {preparingTools ? "正在准备内置工具…" : "一键准备内置工具"}
                </button>
              )}
            </div>

            <div className="card guard-card">
              <div className="guard-title"><ShieldCheck size={19} /><h3>确定性保护</h3></div>
              <ul>
                <li><Check size={14} />AI 调用只发生在准备阶段</li>
                <li><Check size={14} />Flow 经校验后生成 SHA-256</li>
                <li><Check size={14} />运行保留原始指标和版本信息</li>
                <li><Check size={14} />模拟结果不会混入真实性能报告</li>
              </ul>
            </div>

            <div className="card architecture-card">
              <p className="eyebrow">EXECUTION MODEL</p>
              <div className="architecture-flow">
                <span><Laptop size={16} />桌面界面</span><ArrowRight size={14} />
                <span><Cpu size={16} />Rust Runner</span><ArrowRight size={14} />
                <span><Database size={16} />原始证据</span>
              </div>
              <p>关闭界面不会改变正式测量逻辑，CLI 与桌面端共用相同核心。</p>
            </div>
          </aside>
        </div>
        </>
        )}
      </main>
    </div>
  );
}

function AnalysisCenter() {
  const [jobs, setJobs] = useState<Job[]>([]);
  const [baselineJobId, setBaselineJobId] = useState("");
  const [currentJobId, setCurrentJobId] = useState("");
  const [analysis, setAnalysis] = useState<JobAnalysis>();
  const [loading, setLoading] = useState(true);
  const [analyzing, setAnalyzing] = useState(false);
  const [error, setError] = useState("");
  const [explanationProvider, setExplanationProvider] = useState<ProviderMode>("offline");
  const [explanation, setExplanation] = useState<AnalysisExplanation>();
  const [explaining, setExplaining] = useState(false);
  const [analysisCliProviders, setAnalysisCliProviders] = useState<CliProviderStatus[]>([]);
  const [analysisLocalStatus, setAnalysisLocalStatus] = useState<LocalModelStatus>();
  const [analysisLocalEndpoint, setAnalysisLocalEndpoint] = useState("http://127.0.0.1:11434");
  const [analysisLocalModel, setAnalysisLocalModel] = useState("qwen2.5:7b");
  const [analysisEndpoint, setAnalysisEndpoint] = useState("https://api.openai.com/v1");
  const [analysisModel, setAnalysisModel] = useState("gpt-5-mini");
  const [analysisApiKey, setAnalysisApiKey] = useState("");
  const [analysisCliModel, setAnalysisCliModel] = useState("");
  const [analysisCodexPath, setAnalysisCodexPath] = useState("");
  const [analysisClaudePath, setAnalysisClaudePath] = useState("");

  async function load() {
    setLoading(true);
    setError("");
    try {
      const page = await listJobs(100, 0);
      const completed = page.jobs.filter((job) => job.state === "completed" && Boolean(job.resultPath));
      setJobs(completed);
      const snapshots = (await Promise.all(completed.slice(0, 20).map(async (job) => {
        try { return await getJobSnapshot(job.id, { limit: 1 }); } catch { return undefined; }
      }))).filter((snapshot): snapshot is JobSnapshot => Boolean(snapshot));
      let suggested: { baseline: string; current: string } | undefined;
      for (let currentIndex = 0; currentIndex < snapshots.length && !suggested; currentIndex += 1) {
        for (let baselineIndex = currentIndex + 1; baselineIndex < snapshots.length; baselineIndex += 1) {
          if (likelyCompatibleResults(snapshots[baselineIndex].results[0], snapshots[currentIndex].results[0])) {
            suggested = { baseline: snapshots[baselineIndex].job.id, current: snapshots[currentIndex].job.id };
            break;
          }
        }
      }
      if (!currentJobId) setCurrentJobId(suggested?.current ?? completed[0]?.id ?? "");
      if (!baselineJobId) setBaselineJobId(suggested?.baseline ?? completed[1]?.id ?? "");
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    void load();
    void doctorCliProviders().then((providers) => {
      setAnalysisCliProviders(providers);
      setAnalysisCodexPath(providers.find((provider) => provider.kind === "codex")?.executable ?? "");
      setAnalysisClaudePath(providers.find((provider) => provider.kind === "claude-code")?.executable ?? "");
    });
    void doctorLocalModel(analysisLocalEndpoint).then((status) => {
      setAnalysisLocalStatus(status);
      if (status.models[0]) setAnalysisLocalModel(status.models[0]);
    });
  }, []);

  async function analyze() {
    if (!baselineJobId || !currentJobId || baselineJobId === currentJobId) return;
    setAnalyzing(true);
    setError("");
    try {
      setAnalysis(await analyzeJobPair(baselineJobId, currentJobId));
      setExplanation(undefined);
    } catch (reason) {
      setAnalysis(undefined);
      setError(String(reason));
    } finally {
      setAnalyzing(false);
    }
  }

  async function explain() {
    const report = analysis?.reports[0];
    if (!report) return;
    setExplaining(true);
    setError("");
    try {
      const isLocal = explanationProvider === "local";
      const isCli = explanationProvider === "codex" || explanationProvider === "claude";
      setExplanation(await explainAnalysis({
        report,
        provider: explanationProvider,
        endpoint: isLocal ? analysisLocalEndpoint : explanationProvider === "cloud" ? analysisEndpoint : undefined,
        apiKey: explanationProvider === "cloud" ? analysisApiKey || undefined : undefined,
        model: isLocal ? analysisLocalModel : isCli ? analysisCliModel || undefined : explanationProvider === "cloud" ? analysisModel : undefined,
        cliExecutable: explanationProvider === "codex" ? analysisCodexPath || undefined : explanationProvider === "claude" ? analysisClaudePath || undefined : undefined,
      }));
    } catch (reason) {
      setError(String(reason));
    } finally {
      setExplaining(false);
    }
  }

  const selectedAnalysisCli = analysisCliProviders.find((provider) => provider.kind === (explanationProvider === "codex" ? "codex" : "claude-code"));
  const explainDisabled = explaining
    || !analysis
    || (explanationProvider === "local" && (!analysisLocalStatus?.available || !analysisLocalModel))
    || (explanationProvider === "cloud" && !analysisApiKey)
    || ((explanationProvider === "codex" || explanationProvider === "claude") && (!selectedAnalysisCli?.available || !selectedAnalysisCli.authenticated));

  return (
    <>
      <header className="topbar analysis-topbar">
        <div><p className="eyebrow">EVIDENCE ANALYSIS</p><h1>结果分析与性能回归</h1></div>
        <div className="top-actions"><span className="status-pill ready"><span className="status-dot" />Rust 规则层 · 无需 AI</span><button className="icon-button" onClick={() => void load()} disabled={loading} aria-label="刷新可分析任务"><RefreshCw size={17} className={loading ? "spin" : ""} /></button></div>
      </header>
      {error && <div className="error-banner">{error}</div>}
      <section className="analysis-picker card">
        <div className="card-heading">
          <div className="heading-icon purple"><CircleGauge size={19} /></div>
          <div><h2>选择两次可比较运行</h2><p>先验证设备、平台、Flow 和指标定义，再执行确定性回归规则</p></div>
          <span className="schema-badge">ANALYSIS v1</span>
        </div>
        <div className="analysis-selectors">
          <label><span>基线运行</span><select value={baselineJobId} onChange={(event) => { setBaselineJobId(event.target.value); setAnalysis(undefined); setExplanation(undefined); }}><option value="">请选择基线</option>{jobs.map((job) => <option value={job.id} key={`baseline-${job.id}`}>{analysisJobLabel(job)}</option>)}</select></label>
          <ArrowRight size={18} />
          <label><span>当前运行</span><select value={currentJobId} onChange={(event) => { setCurrentJobId(event.target.value); setAnalysis(undefined); setExplanation(undefined); }}><option value="">请选择当前运行</option>{jobs.map((job) => <option value={job.id} key={`current-${job.id}`}>{analysisJobLabel(job)}</option>)}</select></label>
          <button className="primary-button" disabled={analyzing || !baselineJobId || !currentJobId || baselineJobId === currentJobId} onClick={() => void analyze()}>{analyzing ? <RefreshCw size={16} className="spin" /> : <CircleGauge size={16} />}开始规则分析</button>
        </div>
        {!loading && jobs.length < 2 && <div className="analysis-empty"><Activity size={20} /><span>至少需要两次已完成且带性能结果的运行。请用同一锁定 Flow 和同一模拟器配置重复测试。</span></div>}
      </section>
      {analysis && <AnalysisReportView analysis={analysis} />}
      {analysis && (
        <section className="analysis-ai card">
          <div className="card-heading"><div className="heading-icon green"><Sparkles size={19} /></div><div><h2>AI 证据解读</h2><p>规则结论不可修改；AI 只解释可能原因并提出验证步骤</p></div><span className="schema-badge">FACTS LOCKED</span></div>
          <div className="analysis-provider-mode" role="group" aria-label="结果解读 Provider">
            {(Object.keys(providerNames) as ProviderMode[]).map((mode) => <button type="button" className={explanationProvider === mode ? "active" : ""} aria-pressed={explanationProvider === mode} key={mode} onClick={() => { setExplanationProvider(mode); setExplanation(undefined); }}>{providerNames[mode]}</button>)}
          </div>
          {explanationProvider === "offline" && <div className="provider-offline"><ShieldCheck size={16} /><div><b>Reactor 规则总结</b><span>不调用模型；只整理已验证事实和下一步复测建议。</span></div></div>}
          {explanationProvider === "local" && <div className="analysis-provider-fields"><div className={`provider-offline ${analysisLocalStatus?.available ? "ready" : "warning"}`}><Cpu size={16} /><div><b>{analysisLocalStatus?.available ? "本地模型已连接" : "本地模型未连接"}</b><span>{analysisLocalStatus?.detail ?? "支持 Ollama / LM Studio"}</span></div></div><label><span>本地地址</span><input value={analysisLocalEndpoint} onChange={(event) => setAnalysisLocalEndpoint(event.target.value)} /></label><label><span>Model</span><input value={analysisLocalModel} onChange={(event) => setAnalysisLocalModel(event.target.value)} /></label></div>}
          {(explanationProvider === "codex" || explanationProvider === "claude") && <div className="analysis-provider-fields"><div className={`provider-offline ${selectedAnalysisCli?.available && selectedAnalysisCli.authenticated ? "ready" : "warning"}`}><ShieldCheck size={16} /><div><b>{providerNames[explanationProvider]} · {selectedAnalysisCli?.version ?? "未检测"}</b><span>{selectedAnalysisCli?.detail ?? "复用本机登录态"}</span></div></div><label><span>可执行文件</span><input value={explanationProvider === "codex" ? analysisCodexPath : analysisClaudePath} onChange={(event) => explanationProvider === "codex" ? setAnalysisCodexPath(event.target.value) : setAnalysisClaudePath(event.target.value)} /></label><label><span>Model（可留空）</span><input value={analysisCliModel} onChange={(event) => setAnalysisCliModel(event.target.value)} /></label></div>}
          {explanationProvider === "cloud" && <div className="analysis-provider-fields"><label><span>Base URL</span><input value={analysisEndpoint} onChange={(event) => setAnalysisEndpoint(event.target.value)} /></label><label><span>Model</span><input value={analysisModel} onChange={(event) => setAnalysisModel(event.target.value)} /></label><label><span>API Key（仅当前会话）</span><input type="password" value={analysisApiKey} onChange={(event) => setAnalysisApiKey(event.target.value)} /></label></div>}
          <div className="analysis-ai-action"><p><LockKeyhole size={14} />回归判定、数值和证据引用由 Rust 锁定</p><button className="primary-button" disabled={explainDisabled} onClick={() => void explain()}>{explaining ? <RefreshCw size={16} className="spin" /> : <Sparkles size={16} />}{explanationProvider === "offline" ? "生成规则总结" : `使用 ${providerNames[explanationProvider]} 解读`}</button></div>
          {explanation && <AnalysisExplanationView explanation={explanation} />}
        </section>
      )}
    </>
  );
}

function AnalysisReportView({ analysis }: { analysis: JobAnalysis }) {
  return (
    <div className="analysis-reports">
      {analysis.reports.map((report) => (
        <section className={`analysis-report card ${report.verdict}`} key={`${report.evidence.framework}-${report.evidence.currentRunId}`}>
          <div className="analysis-verdict">
            <div><span className={`verdict-dot ${report.verdict}`} /><div><b>{frameworkNames[report.evidence.framework] ?? report.evidence.framework} · {analysisVerdictLabel(report.verdict)}</b><small>{report.evidence.platform} · {report.evidence.scenario} · {report.evidence.deviceClass === "physical" ? "物理设备" : "模拟器"}</small></div></div>
            <code>Flow {report.evidence.flowHash.slice(0, 12)}…</code>
          </div>
          {!report.compatibility.compatible ? (
            <div className="compatibility-block incompatible"><ShieldCheck size={18} /><div><b>基线不兼容，已拒绝给出回归结论</b>{report.compatibility.reasons.map((reason) => <span key={reason}>{reason}</span>)}</div></div>
          ) : (
            <div className="compatibility-block"><Check size={18} /><div><b>基线兼容性检查通过</b><span>平台、设备类别、Flow、场景和指标定义一致</span>{report.compatibility.warnings.map((warning) => <span key={warning}>{warning}</span>)}</div></div>
          )}
          <div className="analysis-findings">
            {report.findings.map((finding) => <article className={`finding ${finding.severity}`} key={finding.id}><div><b>{finding.title}</b><span className="fact-badge">事实 · 规则层</span></div><p>{finding.summary}</p><small>证据：{finding.evidenceRefs.join(" · ")}</small></article>)}
          </div>
          <div className="metric-diff-table">
            <div className="metric-diff-row header"><span>指标</span><span>基线</span><span>当前</span><span>变化</span><span>判定</span></div>
            {report.metrics.map((metric) => <div className={`metric-diff-row ${metric.verdict}`} key={metric.id}><span><b>{metric.label}</b><small>阈值 {metric.thresholdPct.toFixed(1)}%</small></span><span>{analysisMetric(metric.baseline, metric.unit)}</span><span>{analysisMetric(metric.current, metric.unit)}</span><span>{metric.percentDelta === undefined ? "—" : `${metric.percentDelta >= 0 ? "+" : ""}${metric.percentDelta.toFixed(1)}%`}</span><span className="metric-verdict">{metricVerdictLabel(metric.verdict)}</span></div>)}
          </div>
          <details className="evidence-details"><summary>查看证据引用</summary><div><b>指标定义</b><span>{report.evidence.metricDefinitions.join(" · ") || "标准 NormalizedResult v1"}</span><b>原始文件</b>{report.evidence.rawEvidence.map((path) => <code key={path}>{path}</code>)}</div></details>
        </section>
      ))}
    </div>
  );
}

function AnalysisExplanationView({ explanation }: { explanation: AnalysisExplanation }) {
  return (
    <div className="analysis-explanation">
      <div className="analysis-explanation-heading"><div><b>{analysisVerdictLabel(explanation.verdict)} · 证据解读</b><span>{explanation.provider} · {explanation.model}</span></div><span className="fact-badge">规则结论未改变</span></div>
      <p className="analysis-summary">{explanation.summary}</p>
      <div className="insight-columns">
        <section><h3>已验证事实</h3>{explanation.facts.map((insight, index) => <article className="insight fact" key={`fact-${index}`}><b>{insight.title}</b><p>{insight.text}</p><small>{insight.evidenceRefs.join(" · ")}</small></article>)}</section>
        <section><h3>可能原因（待验证）</h3>{explanation.hypotheses.length ? explanation.hypotheses.map((insight, index) => <article className="insight hypothesis" key={`hypothesis-${index}`}><b>{insight.title}</b><p>{insight.text}</p><small>{insight.evidenceRefs.join(" · ")}</small></article>) : <div className="insight-empty">当前 Provider 未生成推测；规则事实不受影响。</div>}</section>
      </div>
      <section className="analysis-next-steps"><h3>建议验证步骤</h3>{explanation.nextSteps.map((step, index) => <div key={`next-${index}`}><span>{index + 1}</span><div><b>{step.title}</b><p>{step.text}</p></div></div>)}</section>
    </div>
  );
}

function analysisJobLabel(job: Job) {
  const meta = jobMetadata(job);
  return `${formatDate(job.createdAt)} · ${meta.framework}/${meta.scenario} · ${job.id.slice(0, 8)}`;
}

function likelyCompatibleResults(baseline?: NormalizedResult, current?: NormalizedResult) {
  if (!baseline || !current || baseline.source.synthetic || current.source.synthetic) return false;
  return baseline.framework === current.framework
    && baseline.platform === current.platform
    && baseline.scenario === current.scenario
    && baseline.flowHash === current.flowHash
    && baseline.device?.id === current.device?.id
    && baseline.device?.physical === current.device?.physical;
}

function analysisVerdictLabel(verdict: JobAnalysis["reports"][number]["verdict"]) {
  return ({ improved: "性能改善", stable: "未发现回归", regressed: "检测到回归", incompatible: "基线不兼容" } as const)[verdict];
}

function metricVerdictLabel(verdict: JobAnalysis["reports"][number]["metrics"][number]["verdict"]) {
  return ({ improved: "改善", stable: "稳定", regressed: "回归", unavailable: "不可用" } as const)[verdict];
}

function analysisMetric(value: number | undefined, unit: string) {
  return value === undefined ? "—" : `${value.toFixed(2)} ${unit}`;
}

function HistoryCenter({
  history,
  selected,
  loading,
  error,
  loadingOlder,
  onRefresh,
  onPage,
  onSelect,
  onLoadOlder,
}: {
  history?: JobPage;
  selected?: JobSnapshot;
  loading: boolean;
  error: string;
  loadingOlder: boolean;
  onRefresh: () => void;
  onPage: (offset: number) => void;
  onSelect: (job: Job) => void;
  onLoadOlder: () => void;
}) {
  const limit = history?.limit ?? 20;
  const offset = history?.offset ?? 0;
  const total = history?.total ?? 0;
  const pageNumber = total ? Math.floor(offset / limit) + 1 : 1;
  const pageCount = Math.max(1, Math.ceil(total / limit));
  const selectedMeta = selected ? jobMetadata(selected.job) : undefined;
  return (
    <>
      <header className="topbar history-topbar">
        <div><p className="eyebrow">RUN HISTORY</p><h1>运行记录与原始证据</h1></div>
        <div className="top-actions">
          <span className="status-pill ready"><span className="status-dot" />本地 SQLite · 可重连</span>
          <button className="icon-button" onClick={onRefresh} disabled={loading} aria-label="刷新运行记录"><RefreshCw size={17} className={loading ? "spin" : ""} /></button>
        </div>
      </header>
      {error && <div className="error-banner">{error}</div>}
      <div className="history-layout">
        <aside className="history-list card" aria-label="运行任务列表">
          <div className="history-list-heading"><div><h2>全部任务</h2><p>{total} 次本地运行</p></div><span>{pageNumber}/{pageCount}</span></div>
          <div className="history-jobs">
            {history?.jobs.map((job) => {
              const meta = jobMetadata(job);
              return (
                <button key={job.id} className={`history-job ${selected?.job.id === job.id ? "active" : ""}`} onClick={() => onSelect(job)}>
                  <span className={`job-state ${job.state}`}>{jobStateNames[job.state]}</span>
                  <b>{meta.framework} · {meta.scenario}</b>
                  <small>{formatDate(job.createdAt)} · {meta.device}</small>
                  <code>{job.id.slice(0, 8)}</code>
                </button>
              );
            })}
            {!loading && history?.jobs.length === 0 && <div className="history-empty">还没有运行记录</div>}
            {loading && !history && <div className="skeleton-list" />}
          </div>
          <div className="history-pagination">
            <button className="secondary-button" disabled={loading || offset === 0} onClick={() => onPage(Math.max(0, offset - limit))}>上一页</button>
            <button className="secondary-button" disabled={loading || offset + limit >= total} onClick={() => onPage(offset + limit)}>下一页</button>
          </div>
        </aside>

        <section className="history-detail">
          {selected && selectedMeta ? (
            <>
              <div className="history-summary card">
                <div className="card-heading">
                  <div className="heading-icon purple"><Activity size={19} /></div>
                  <div><h2>{selectedMeta.framework} · {selectedMeta.scenario}</h2><p>任务 {selected.job.id}</p></div>
                  <span className={`job-state ${selected.job.state}`}>{jobStateNames[selected.job.state]}</span>
                </div>
                <div className="history-facts">
                  <div><span>应用 / 模式</span><b>{selectedMeta.app}</b></div>
                  <div><span>设备</span><b>{selectedMeta.device}</b></div>
                  <div><span>开始时间</span><b>{formatDate(selected.job.createdAt)}</b></div>
                  <div><span>更新时间</span><b>{formatDate(selected.job.updatedAt)}</b></div>
                </div>
                {selected.job.error && <p className="run-error">{selected.job.error}</p>}
              </div>

              <div className="history-events card">
                <div className="history-section-heading">
                  <div><h2>执行时间线</h2><p>已加载 {selected.events.length} 条；列表只渲染可见行</p></div>
                  {selected.hasMoreEvents && <button className="secondary-button" disabled={loadingOlder} onClick={onLoadOlder}>{loadingOlder ? "加载中…" : "加载更早事件"}</button>}
                </div>
                <VirtualEventList events={selected.events} />
              </div>

              {selected.results.length > 0 && <Results results={selected.results} reportPath={selected.reportPath ?? ""} />}
            </>
          ) : (
            <div className="history-empty-detail card"><Activity size={26} /><b>选择一条运行记录</b><span>查看阶段事件、性能指标和 HTML 报告</span></div>
          )}
        </section>
      </div>
    </>
  );
}

function VirtualEventList({ events }: { events: JobSnapshot["events"] }) {
  const rowHeight = 58;
  const viewportHeight = 348;
  const overscan = 4;
  const [scrollTop, setScrollTop] = useState(0);
  const start = Math.max(0, Math.floor(scrollTop / rowHeight) - overscan);
  const visibleCount = Math.ceil(viewportHeight / rowHeight) + overscan * 2;
  const visible = events.slice(start, start + visibleCount);
  return (
    <div className="event-viewport" style={{ height: viewportHeight }} onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}>
      <div className="event-spacer" style={{ height: events.length * rowHeight }}>
        <div className="event-window" style={{ transform: `translateY(${start * rowHeight}px)` }}>
          {visible.map((event) => (
            <div className="history-event" style={{ height: rowHeight }} key={event.id}>
              <span className={`event-dot ${event.phase}`} />
              <div><b>{jobStateNames[event.phase]}</b><p>{event.message}</p></div>
              <time>{formatTime(event.createdAt)}</time>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

function RunStatus({
  snapshot,
  cancelling,
  onCancel,
}: {
  snapshot: JobSnapshot;
  cancelling: boolean;
  onCancel: () => void;
}) {
  const terminal = ["completed", "failed", "cancelled"].includes(snapshot.job.state);
  const latestEvents = snapshot.events.slice(-4);
  return (
    <section className={`run-status card ${terminal ? "terminal" : "active"}`} aria-live="polite">
      <div className="run-status-heading">
        <div className="heading-icon purple"><Activity size={19} /></div>
        <div><h2>{jobStateNames[snapshot.job.state]}</h2><p>任务 {snapshot.job.id.slice(0, 8)} · 重启 Reactor 后可自动重连</p></div>
        {!terminal && <span className="running-indicator"><span />Runner 运行中</span>}
        {!terminal && <button className="secondary-button danger-button" disabled={cancelling} onClick={onCancel}>{cancelling ? "正在取消…" : "取消任务"}</button>}
      </div>
      {latestEvents.length > 0 && (
        <div className="run-events">
          {latestEvents.map((event) => <div key={event.id}><span>{jobStateNames[event.phase]}</span><p>{event.message}</p></div>)}
        </div>
      )}
      {snapshot.job.error && <p className="run-error">{snapshot.job.error}</p>}
    </section>
  );
}

function PreparationReview({ preparation }: { preparation: TrialPreparation }) {
  const preview = preparation.context?.preview;
  if (preparation.failure) {
    return (
      <div className="preparation-review failed">
        <div className="review-heading"><ShieldCheck size={17} /><div><b>试跑失败，已保留本地证据</b><span>{preparation.failure.message.slice(0, 240)}</span></div></div>
        {preview && (
          <div className="privacy-preview">
            <div><b>{preview.elementCount}</b><span>UI 元素</span></div>
            <div><b>{preview.redactionCount}</b><span>敏感值已脱敏</span></div>
            <div><b>{preview.includedChars}</b><span>将提供的字符</span></div>
            <div><b>0 B</b><span>截图上传</span></div>
          </div>
        )}
        <p className="privacy-copy">点击“AI 自愈”只会发送上述脱敏 UI 字段和失败摘要；截图、原始 UI 树与源码保留在本机。</p>
      </div>
    );
  }
  if (preparation.trial?.synthetic) {
    return (
      <div className="preparation-review failed">
        <div className="review-heading"><ShieldCheck size={17} /><div><b>仅静态校验，未在目标上运行</b><span>此结果不能锁定 Flow 或用于性能测试。请连接目标后重新试跑。</span></div></div>
      </div>
    );
  }
  return (
    <div className="preparation-review passed">
      <div className="review-heading"><Check size={17} /><div><b>试跑已通过</b><span>{preparation.goalEvidence?.verified ? `已证明目标页标记“${preparation.goalEvidence.marker}”仅出现在导航后页面` : preparation.repairAttempts ? `AI 修复 ${preparation.repairAttempts} 次 · 需人工确认 diff` : "未调用模型 · 确认后才会锁定"}</span></div></div>
      {preparation.goalEvidence?.verified && (
        <div className="privacy-preview">
          <div><b>{preparation.goalEvidence.sourceElements}</b><span>起始页元素</span></div>
          <div><b>{preparation.goalEvidence.destinationElements}</b><span>目标页元素</span></div>
          <div><b>否</b><span>起始页含标记</span></div>
          <div><b>是</b><span>目标页含标记</span></div>
        </div>
      )}
      {preparation.changes.length > 0 && (
        <div className="flow-diff">
          {preparation.changes.slice(0, 12).map((change) => (
            <div key={change.path}><code>{change.path}</code><span>{compactValue(change.before)} → {compactValue(change.after)}</span></div>
          ))}
        </div>
      )}
      {preparation.auditPath && <p className="privacy-copy">修复审计已保存；正式测量窗口模型调用数固定为 0。</p>}
    </div>
  );
}

function compactValue(value: unknown) {
  if (value === undefined) return "∅";
  const serialized = JSON.stringify(value);
  return serialized.length > 72 ? `${serialized.slice(0, 69)}…` : serialized;
}

function loadFlowDraft(): PersistedFlowDraft | undefined {
  try {
    const raw = window.localStorage.getItem(FLOW_DRAFT_KEY);
    if (!raw) return undefined;
    const value = JSON.parse(raw) as Partial<PersistedFlowDraft>;
    if (
      value.version !== 1
      || typeof value.intent !== "string"
      || typeof value.appId !== "string"
      || !["react-native", "flutter", "lynx"].includes(value.framework ?? "")
      || !["android", "ios"].includes(value.platform ?? "")
      || !["offline", "codex", "claude", "cloud"].includes(value.providerMode ?? "")
      || !["quick", "standard"].includes(value.runPreset ?? "")
    ) return undefined;
    return value as PersistedFlowDraft;
  } catch {
    return undefined;
  }
}

function jobMetadata(job: Job) {
  const request = typeof job.request === "object" && job.request !== null
    ? job.request as Record<string, unknown>
    : {};
  const mode = typeof request.mode === "string" ? request.mode : "measurement";
  if (mode === "demo") {
    return { framework: "三框架", scenario: "模拟导览", device: "无设备 · 虚拟数据", app: "产品布局预览（非测量）" };
  }
  const framework = typeof request.framework === "string" ? request.framework : "unknown";
  const scenario = typeof request.scenario === "string" ? request.scenario : "custom";
  const device = typeof request.deviceId === "string" ? request.deviceId : "未知设备";
  return {
    framework: frameworkNames[framework] ?? framework,
    scenario,
    device,
    app: "锁定 Flow · 真实采集",
  };
}

function formatDate(value: string) {
  return new Intl.DateTimeFormat("zh-CN", {
    month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", second: "2-digit",
  }).format(new Date(value));
}

function formatTime(value: string) {
  return new Intl.DateTimeFormat("zh-CN", {
    hour: "2-digit", minute: "2-digit", second: "2-digit",
  }).format(new Date(value));
}

function isTerminal(state: JobState) {
  return state === "completed" || state === "failed" || state === "cancelled";
}

function mergeEvents(left: JobSnapshot["events"], right: JobSnapshot["events"]) {
  return [...new Map([...left, ...right].map((event) => [event.id, event])).values()]
    .sort((a, b) => a.id - b.id);
}

function maestroSource(compiled?: CompiledFlow) {
  if (!compiled) return "# Maestro 编译结果尚未生成";
  return [
    "# setup.yaml",
    compiled.setup.trimEnd(),
    "",
    "# measured.yaml",
    compiled.measured.trimEnd(),
    "",
    "# teardown.yaml",
    compiled.teardown.trimEnd(),
  ].join("\n");
}

function FlowSource({
  kind,
  source,
  copied,
  onCopy,
  onEdit,
}: {
  kind: Exclude<FlowView, "steps">;
  source: string;
  copied: boolean;
  onCopy: () => void;
  onEdit?: () => void;
}) {
  return (
    <div className="flow-source">
      <div className="flow-source-heading">
        <div><b>{kind === "json" ? "Reactor Flow JSON" : "Maestro 执行 YAML"}</b><span>{kind === "json" ? "校验、锁定和计算 SHA-256 的完整输入" : "由 Rust 编译器生成，试跑和正式测量执行同一语义"}</span></div>
        <div className="flow-source-actions">
          {onEdit && <button className="secondary-button" onClick={onEdit}><Pencil size={14} />编辑 JSON</button>}
          <button className="secondary-button" onClick={onCopy}>{copied ? "已复制" : "复制完整内容"}</button>
        </div>
      </div>
      <pre><code>{source}</code></pre>
    </div>
  );
}

function FlowTimeline({ title, steps, muted = false }: { title: string; steps: FlowStep[]; muted?: boolean }) {
  return (
    <div className={`flow-section ${muted ? "muted" : ""}`}>
      <div className="flow-section-title"><span />{title}<b>{steps.length} 步</b></div>
      <div className="flow-steps">
        {steps.map((step, index) => (
          <div className="flow-step" key={`${step.action}-${index}`}>
            <span className="step-index">{index + 1}</span>
            <div><b>{stepNames[step.action]}</b><small>{stepDetail(step)}</small></div>
            {step.action === "repeat" && <span className="repeat-badge">× {step.times}</span>}
          </div>
        ))}
      </div>
    </div>
  );
}

function DeviceLab({
  environment,
  selectedDeviceId,
  refreshing,
  preparingTools,
  onRefresh,
  onPrepareTools,
  onUseDevice,
}: {
  environment?: Bootstrap;
  selectedDeviceId: string;
  refreshing: boolean;
  preparingTools: boolean;
  onRefresh: () => void;
  onPrepareTools: () => void;
  onUseDevice: (deviceId: string, platform: string) => void;
}) {
  const devices = environment?.devices ?? [];
  const androidCount = devices.filter((device) => device.platform === "android").length;
  const iosCount = devices.filter((device) => device.platform === "ios").length;
  const physicalCount = devices.filter((device) => device.physical).length;
  return (
    <>
      <header className="topbar">
        <div><p className="eyebrow">DEVICE LAB</p><h1>设备实验室</h1></div>
        <div className="top-actions">
          <span className={`status-pill ${environment?.doctor.ready ? "ready" : "waiting"}`}><span className="status-dot" />{environment?.doctor.ready ? "采集环境已就绪" : "工具链需要准备"}</span>
          <button className="secondary-button" onClick={onRefresh} disabled={refreshing}>{refreshing ? <RefreshCw size={16} className="spin" /> : <RefreshCw size={16} />}刷新设备</button>
        </div>
      </header>

      <section className="device-stats" aria-label="设备概览">
        <article><Smartphone size={18} /><span>可用目标</span><strong>{devices.length}</strong></article>
        <article><Bot size={18} /><span>Android</span><strong>{androidCount}</strong></article>
        <article><Laptop size={18} /><span>iOS Simulator</span><strong>{iosCount}</strong></article>
        <article><Cpu size={18} /><span>物理设备</span><strong>{physicalCount}</strong></article>
      </section>

      <div className="device-lab-grid">
        <section className="card device-list-card">
          <div className="card-heading"><div className="heading-icon purple"><Smartphone size={18} /></div><div><h2>已发现设备</h2><p>模拟器与物理设备始终分组记录，不会混排性能结论。</p></div></div>
          {devices.length === 0 ? (
            <div className="empty-state"><Smartphone size={28} /><h3>尚未发现设备</h3><p>启动 Android Emulator 或 iOS Simulator 后点击“刷新设备”。</p></div>
          ) : (
            <div className="device-list">
              {devices.map((device) => (
                <article className={`device-row ${device.id === selectedDeviceId ? "selected" : ""}`} key={`${device.platform}-${device.id}`}>
                  <div className={`device-platform ${device.platform}`}><Smartphone size={18} /></div>
                  <div className="device-main">
                    <div><strong>{device.name ?? device.id}</strong><span className={`device-state ${device.state === "device" || device.state === "booted" ? "ready" : ""}`}>{device.state}</span></div>
                    <code>{device.id}</code>
                    <p>{device.platform === "ios" ? "iOS" : "Android"} · {device.physical ? "物理设备" : "模拟器"} · {device.metadata.osVersion ?? "OS 未知"} · {device.metadata.refreshRate ? `${device.metadata.refreshRate} Hz` : "刷新率待测"}</p>
                  </div>
                  <button className="secondary-button" onClick={() => onUseDevice(device.id, device.platform)}>用于 Flow Studio</button>
                </article>
              ))}
            </div>
          )}
        </section>

        <aside className="card device-tools-card">
          <div className="card-heading"><div className="heading-icon orange"><HardDrive size={18} /></div><div><h2>受管工具链</h2><p>无需单独安装 Maestro。</p></div></div>
          <div className="tool-list">
            {(environment?.doctor.checks ?? []).map((check) => (
              <div className="tool-row" key={check.id}><div className={`tool-icon ${check.available ? "ok" : "missing"}`}>{check.available ? <Check size={15} /> : <Database size={15} />}</div><div><strong>{check.label}</strong><span>{check.available ? "已就绪" : "缺失"}{check.detail ? ` · ${check.detail}` : ""}</span></div></div>
            ))}
          </div>
          {!environment?.doctor.ready && <button className="primary-button full-width" onClick={onPrepareTools} disabled={preparingTools}>{preparingTools ? <RefreshCw size={16} className="spin" /> : <HardDrive size={16} />}准备受管工具</button>}
          <div className="lab-policy"><ShieldCheck size={16} /><span>AI 不进入测量窗口；设备类别、OS 和刷新率写入每次结果。</span></div>
        </aside>
      </div>
    </>
  );
}

function SettingsCenter({
  environment,
  cliProviders,
  localModelStatus,
  checkingCli,
  checkingLocalModel,
  preparingTools,
  onRefreshCli,
  onRefreshLocal,
  onPrepareTools,
}: {
  environment?: Bootstrap;
  cliProviders: CliProviderStatus[];
  localModelStatus?: LocalModelStatus;
  checkingCli: boolean;
  checkingLocalModel: boolean;
  preparingTools: boolean;
  onRefreshCli: () => void;
  onRefreshLocal: () => void;
  onPrepareTools: () => void;
}) {
  const [maintenance, setMaintenance] = useState<MaintenanceStatus>();
  const [maintenanceBusy, setMaintenanceBusy] = useState(false);
  const [maintenanceNotice, setMaintenanceNotice] = useState("");
  const [maintenanceError, setMaintenanceError] = useState("");
  const [stagedUpdate, setStagedUpdate] = useState<StagedUpdate>();
  const [updateChannel, setUpdateChannel] = useState<"stable" | "beta">("stable");

  async function refreshMaintenance() {
    setMaintenanceBusy(true);
    setMaintenanceError("");
    try {
      setMaintenance(await getMaintenanceStatus());
    } catch (reason) {
      setMaintenanceError(String(reason));
    } finally {
      setMaintenanceBusy(false);
    }
  }

  useEffect(() => {
    void refreshMaintenance();
  }, []);

  async function exportDiagnostic() {
    setMaintenanceBusy(true);
    setMaintenanceError("");
    setMaintenanceNotice("");
    try {
      const bundle = await createDiagnosticBundle();
      setMaintenanceNotice(`诊断包已生成：${bundle.path}`);
      await refreshMaintenance();
    } catch (reason) {
      setMaintenanceError(String(reason));
      setMaintenanceBusy(false);
    }
  }

  async function eraseSensitiveArtifacts() {
    if (!window.confirm("删除本机保存的试跑截图和 UI 树？原始性能 trace、报告和运行历史会保留。")) return;
    setMaintenanceBusy(true);
    setMaintenanceError("");
    setMaintenanceNotice("");
    try {
      const result = await erasePrivateData("sensitive_artifacts");
      setMaintenanceNotice(`已擦除 ${result.removedFiles} 个敏感 artifact（${formatBytes(result.removedBytes)}）`);
      await refreshMaintenance();
    } catch (reason) {
      setMaintenanceError(String(reason));
      setMaintenanceBusy(false);
    }
  }

  async function eraseAllLocalData() {
    if (!window.confirm("这会删除全部运行历史、结果、报告、Flow 草稿和已保存的 Cloud API Key。受管工具会保留，操作不可撤销。是否继续？")) return;
    if (!window.confirm("最后确认：确定清空 Reactor 的全部本地测试数据？")) return;
    setMaintenanceBusy(true);
    setMaintenanceError("");
    try {
      await erasePrivateData("all_local_data");
      window.localStorage.removeItem(FLOW_DRAFT_KEY);
      window.location.reload();
    } catch (reason) {
      setMaintenanceError(String(reason));
      setMaintenanceBusy(false);
    }
  }

  async function checkAndStageUpdate() {
    setMaintenanceBusy(true);
    setMaintenanceError("");
    setMaintenanceNotice("");
    try {
      const staged = await stageUpdate(updateChannel);
      setStagedUpdate(staged);
      setMaintenanceNotice(`Reactor ${staged.version} 已完成签名、兼容性、大小和 SHA-256 校验，等待重启安装。`);
    } catch (reason) {
      setMaintenanceError(String(reason));
    } finally {
      setMaintenanceBusy(false);
    }
  }

  async function installUpdate() {
    if (!stagedUpdate) return;
    if (!window.confirm(`安装 Reactor ${stagedUpdate.version}？应用会退出，候选版本健康检查失败时自动恢复当前版本。`)) return;
    setMaintenanceBusy(true);
    setMaintenanceError("");
    try {
      await installStagedUpdate(stagedUpdate.transactionPath);
    } catch (reason) {
      setMaintenanceError(String(reason));
      setMaintenanceBusy(false);
    }
  }

  return (
    <>
      <header className="topbar"><div><p className="eyebrow">SETTINGS</p><h1>设置与能力诊断</h1></div><span className="status-pill ready"><span className="status-dot" />本地优先</span></header>
      <div className="settings-grid">
        <section className="card"><div className="card-heading"><div className="heading-icon purple"><Bot size={18} /></div><div><h2>AI Provider</h2><p>只复用已有安装和登录态，不读取凭据。</p></div><button className="icon-button" onClick={onRefreshCli} disabled={checkingCli}><RefreshCw size={16} className={checkingCli ? "spin" : ""} /></button></div>
          <div className="settings-list">{cliProviders.map((provider) => <div key={provider.kind}><span>{provider.label}</span><b className={provider.available && provider.authenticated ? "ok-text" : "muted-text"}>{provider.available ? provider.authenticated ? "可用" : "待登录" : "未安装"}</b><small>{provider.version ?? provider.detail}</small></div>)}</div>
          <div className="settings-list"><div><span>Local Model（可选）</span><b className={localModelStatus?.available ? "ok-text" : "muted-text"}>{localModelStatus?.available ? "可用" : "未运行"}</b><small>{localModelStatus?.detail ?? "未检测"}</small><button className="text-button" onClick={onRefreshLocal} disabled={checkingLocalModel}>重新检测</button></div><div><span>Cloud API（可选）</span><b className="muted-text">按需配置</b><small>API Key 仅存系统钥匙串；无 Key 时禁止调用。</small></div></div>
        </section>
        <section className="card"><div className="card-heading"><div className="heading-icon orange"><HardDrive size={18} /></div><div><h2>运行环境</h2><p>{environment?.workspace ?? "正在读取工作区"}</p></div></div>
          <div className="settings-list">{(environment?.doctor.checks ?? []).map((check) => <div key={check.id}><span>{check.label}</span><b className={check.available ? "ok-text" : "muted-text"}>{check.available ? "已就绪" : "缺失"}</b><small>{check.detail ?? (check.managed ? "由 Reactor 管理" : "系统能力")}</small></div>)}</div>
          <button className="secondary-button full-width" onClick={onPrepareTools} disabled={preparingTools}>{preparingTools ? <RefreshCw size={16} className="spin" /> : <HardDrive size={16} />}检查并准备工具链</button>
        </section>
        <section className="card"><div className="card-heading"><div className="heading-icon green"><ShieldCheck size={18} /></div><div><h2>发布加固与资源策略</h2><p>所有限制由 Reactor 核心强制，而不是仅作为界面提示。</p></div><button className="icon-button" onClick={() => void refreshMaintenance()} disabled={maintenanceBusy}><RefreshCw size={16} className={maintenanceBusy ? "spin" : ""} /></button></div>
          <div className="settings-list">
            <div><span>数据库兼容门禁</span><b className="ok-text">Schema v{maintenance?.schemaVersion ?? "—"}</b><small>升级使用事务；未来版本数据库会被只读拒绝，不会被旧版覆盖。</small></div>
            <div><span>应用更新</span><b className={maintenance?.update.productionKeyConfigured ? "ok-text" : "muted-text"}>v{maintenance?.update.currentVersion ?? "—"} · {updateChannel}</b><small>Manifest v{maintenance?.update.manifestSchemaVersion ?? "—"} · {maintenance?.update.signatureAlgorithm ?? "Ed25519"} 签名必需 · 分阶段安装 · 候选版本健康探针失败自动回滚 App 与数据库。{maintenance?.update.productionKeyConfigured ? "当前构建已配置发布公钥。" : "当前为开发构建，正式发布公钥由 CI 注入；不会接受未签名更新。"}</small><select value={updateChannel} onChange={(event) => { setUpdateChannel(event.target.value as "stable" | "beta"); setStagedUpdate(undefined); }} disabled={maintenanceBusy}><option value="stable">Stable 稳定通道</option><option value="beta">Beta 预览通道</option></select><button className="text-button" onClick={() => void checkAndStageUpdate()} disabled={maintenanceBusy}>{maintenanceBusy ? "正在检查…" : "检查并暂存更新"}</button>{stagedUpdate && <button className="text-button" onClick={() => void installUpdate()} disabled={maintenanceBusy}>重启安装 v{stagedUpdate.version}</button>}</div>
            {maintenance?.lastUpdate && <div><span>最近更新事务</span><b className={maintenance.lastUpdate.phase === "healthy" ? "ok-text" : maintenance.lastUpdate.phase === "rolled_back" || maintenance.lastUpdate.phase === "quarantined" ? "muted-text" : ""}>v{maintenance.lastUpdate.version} · {maintenance.lastUpdate.phase}</b><small>{maintenance.lastUpdate.error ?? `创建于 ${formatDate(maintenance.lastUpdate.createdAt)}`}</small></div>}
            <div><span>稳定版兼容承诺</span><b className="ok-text">1.x</b><small>{maintenance?.update.compatibilityLine ?? "Flow v1、Result v1 与历史数据库在 1.x 内保持可读；破坏性变化只进入新的主版本。"}</small></div>
            <div><span>适配器信任策略</span><b className="ok-text">仅内置</b><small>契约 v{maintenance?.policy.pluginContractVersion ?? "—"} · 外部插件默认禁用 · {(maintenance?.policy.trustedBuiltInAdapters ?? []).join(" / ") || "读取中"}</small></div>
            <div><span>AI CLI 上限</span><b>{maintenance?.policy.aiCliTimeoutSeconds ?? "—"} 秒</b><small>stdout {formatBytes(maintenance?.policy.aiCliStdoutBytes)} · stderr {formatBytes(maintenance?.policy.aiCliStderrBytes)} · 超时终止整个进程组。</small></div>
            <div><span>Profile 导入上限</span><b>{formatBytes(maintenance?.policy.maxProfileJsonBytes)}</b><small>Source Map {formatBytes(maintenance?.policy.maxSourceMapBytes)} · Trace 前置可用空间至少 {formatBytes(maintenance?.policy.localTraceMinFreeBytes)}。</small></div>
          </div>
        </section>
        <section className="card privacy-card"><div className="card-heading"><div className="heading-icon purple"><FileDown size={18} /></div><div><h2>诊断与隐私</h2><p>{maintenance ? `${maintenance.historyCount} 条历史 · 本地数据 ${formatBytes(maintenance.workspaceBytes)} · 可用磁盘 ${formatBytes(maintenance.availableDiskBytes)}` : "正在检查本地数据"}</p></div></div>
          <div className="privacy-policy"><ShieldCheck size={16} /><span>诊断包默认不包含凭据值、任务输入、错误正文、绝对路径、截图或 UI 树。</span></div>
          <div className="privacy-actions">
            <button className="secondary-button" onClick={() => void exportDiagnostic()} disabled={maintenanceBusy}><FileDown size={16} />生成安全诊断包</button>
            <button className="secondary-button" onClick={() => void eraseSensitiveArtifacts()} disabled={maintenanceBusy || maintenance?.sensitiveArtifactCount === 0}><Trash2 size={16} />擦除截图与 UI 树（{maintenance?.sensitiveArtifactCount ?? 0}）</button>
            <button className="secondary-button danger-button" onClick={() => void eraseAllLocalData()} disabled={maintenanceBusy}><Trash2 size={16} />清空全部本地测试数据</button>
          </div>
          {maintenanceNotice && <p className="maintenance-notice">{maintenanceNotice}</p>}
          {maintenanceError && <p className="maintenance-error">{maintenanceError}</p>}
        </section>
      </div>
    </>
  );
}

function formatBytes(value?: number) {
  if (value === undefined) return "—";
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KiB`;
  if (value < 1024 * 1024 * 1024) return `${(value / 1024 / 1024).toFixed(1)} MiB`;
  return `${(value / 1024 / 1024 / 1024).toFixed(2)} GiB`;
}

function stepDetail(step: FlowStep) {
  if ("target" in step) return step.target.accessibilityId ?? step.target.semanticId ?? step.target.text ?? "坐标选择器";
  if (step.action === "swipe") return `${step.direction.toUpperCase()} · ${step.duration_ms} ms`;
  if (step.action === "repeat") return `${step.steps.length} 个操作`;
  if (step.action === "pause") return `${step.duration_ms} ms`;
  return "由 Reactor 确定性执行";
}

function Results({ results, reportPath }: { results: NormalizedResult[]; reportPath: string }) {
  const maxCpu = Math.max(...results.map((item) => item.summary.cpuMeanPct ?? 0), 1);
  const synthetic = results.some((item) => item.source.synthetic);
  const emulator = !synthetic && results.some((item) => item.device?.physical === false);
  const simulatorLabel = results[0]?.platform === "ios" ? "iOS Simulator" : "Android 模拟器";
  return (
    <div className="results-card card">
      <div className="card-heading">
        <div className="heading-icon orange"><CircleGauge size={19} /></div>
        <div><h2>性能结果</h2><p>{synthetic ? "完整流程已跑通 · 当前为明确标记的产品导览数据" : emulator ? `${simulatorLabel}测量 · 可用于同机开发回归` : "物理设备测量 · 指标可追溯到原始证据"}</p></div>
        <span className={synthetic ? "synthetic-badge" : "schema-badge"}>{synthetic ? "SIMULATED" : emulator ? "EMULATOR" : "MEASURED"}</span>
      </div>
      <div className="metric-grid">
        {results.map((result) => (
          <article className="metric-card" key={result.framework}>
            <div className="metric-title"><span className={`framework-dot ${result.framework}`} />{frameworkNames[result.framework] ?? result.framework}</div>
            {(result.appVersion || result.device?.osVersion) && (
              <div className="metric-context">
                <span>{result.appVersion ?? "应用版本未记录"}</span>
                <span>{result.device?.name ?? result.device?.id ?? "设备未记录"} · {result.device?.osVersion ?? "OS 未记录"}</span>
              </div>
            )}
            <strong>{formatMetric(result.androidNative?.frameTimeP95Ms ?? result.iosNative?.cpuMeanPct ?? result.summary.fpsMean)}</strong><small>{result.androidNative ? "P95 帧耗时 (ms)" : result.iosNative ? "Time Profiler CPU (%)" : "平均 FPS"}</small>
            {result.androidNative ? (
              <>
                <div className="metric-row"><span>P50 / P99</span><b>{formatMetric(result.androidNative.frameTimeP50Ms)} / {formatMetric(result.androidNative.frameTimeP99Ms)} ms</b></div>
                <div className="metric-row"><span>Jank</span><b>{formatMetric(result.androidNative.jankFramePct)}% · {result.androidNative.jankFrameCount}/{result.androidNative.frameCount}</b></div>
                <div className="metric-row"><span>超帧预算</span><b>{formatMetric(result.androidNative.overBudgetFramePct)}%</b></div>
                <div className="metric-row"><span>冷启动</span><b>{formatMetric(result.androidNative.startupTimeMs)} ms</b></div>
                <div className="metric-row"><span>原生 PSS</span><b>{formatMetric(result.androidNative.memoryPssMb)} MB</b></div>
                <div className="metric-row"><span>热状态</span><b>{formatThermal(result.androidNative.thermalStatusBefore)} → {formatThermal(result.androidNative.thermalStatusAfter)}</b></div>
                <div className="native-evidence">{result.androidNative.collector} · TP {result.androidNative.traceProcessorVersion}</div>
              </>
            ) : result.iosNative ? (
              <>
                <div className="metric-row"><span>Running 样本</span><b>{result.iosNative.cpuSampleCount}</b></div>
                <div className="metric-row"><span>录制时长</span><b>{formatMetric(result.iosNative.recordingDurationMs)} ms</b></div>
                <div className="metric-row"><span>帧</span><b>{availabilityLabel(result.iosNative.availability.frames)}</b></div>
                <div className="metric-row"><span>启动</span><b>{availabilityLabel(result.iosNative.availability.startup)}</b></div>
                <div className="metric-row"><span>内存 / 能耗</span><b>{availabilityLabel(result.iosNative.availability.memory)}</b></div>
                <div className="native-evidence">{result.iosNative.collector} · xctrace {result.iosNative.xctraceVersion}</div>
              </>
            ) : (
              <>
                <div className="metric-row"><span>P10 FPS</span><b>{formatMetric(result.summary.fpsP10)}</b></div>
                <div className="metric-row"><span>平均内存</span><b>{formatMetric(result.summary.ramMeanMb)} MB</b></div>
              </>
            )}
            <div className="cpu-bar"><span style={{ width: `${((result.summary.cpuMeanPct ?? 0) / maxCpu) * 100}%` }} /></div>
            <div className="metric-row subtle"><span>CPU</span><b>{formatMetric(result.summary.cpuMeanPct)}%</b></div>
          </article>
        ))}
      </div>
      <div className="result-warning"><ShieldCheck size={16} /><span>{synthetic ? "这组数据只用于体验产品交互。启动模拟器后，Reactor 将使用受管 Maestro 与原生采集器生成可追溯报告。" : emulator ? `结果来自 ${simulatorLabel}，只能与同一主机、同一模拟器配置的结果比较；不支持的指标不会输出占位值。` : "结果由锁定 Flow 在物理设备执行，原始采集文件和 Flow 哈希已经保存。"}</span></div>
      {reportPath && <div className="flow-actions"><p>独立 HTML 报告与原始结果已保存。</p><button className="secondary-button" onClick={() => openReport(reportPath)}><HardDrive size={16} />打开完整报告</button></div>}
    </div>
  );
}

function formatMetric(value?: number) {
  return value === undefined ? "—" : value.toFixed(1);
}

function formatThermal(value?: number) {
  return value === undefined ? "—" : String(value);
}

function availabilityLabel(value: string) {
  if (value.startsWith("available")) return "可用";
  if (value.startsWith("not_claimed")) return "未宣称";
  return "Simulator 不支持";
}

export default App;
