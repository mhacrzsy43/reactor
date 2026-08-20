import {
  Activity,
  AlertTriangle,
  Braces,
  Check,
  ChevronRight,
  Cpu,
  GitCompare,
  Layers3,
  MapPinned,
  RefreshCw,
  Timer,
  Upload,
} from "lucide-react";
import { Component, useEffect, useMemo, useRef, useState } from "react";
import type { ChangeEvent, ReactNode } from "react";
import { analyzeManagedProfile, analyzeProfileJson, diffProfileReports, getDiagnosticRerunEligibility, listDiagnosticRuns, loadHistoricalFlowLock } from "./api";
import { diagnosticRunIdentity, diagnosticWorkbenchKey, groupDiagnosticRunsByFlow, historicalRerunBlockingReferences, preferredDiagnosticFlowHash, preferredDiagnosticRun, RequestTokens, sourceMapStatus } from "./diagnosticLogic";
import { UnifiedTimeline } from "./UnifiedTimeline";
import type {
  ComponentProfileStat,
  DiagnosticProfileReport,
  DiagnosticArtifactRef,
  DiagnosticRerunEligibility,
  DiagnosticRunSummary,
  FlowLock,
  NormalizedResult,
  ProfileCommit,
  ProfileDiffReport,
  ProfileEvidence,
  ProfileEvidenceKind,
  SourceLocation,
  SourceMapEvidence,
} from "./types";

type DiagnosticView = "overview" | "runtime" | "render" | "findings" | "hermes" | "timeline" | "diff" | "source";
type DiagnosticFramework = "react-native" | "flutter" | "lynx";

interface DiagnosticFlowContext {
  flowHash: string;
  name: string;
  appId: string;
  framework: DiagnosticFramework;
}

interface DiagnosticCenterProps {
  activeFlow?: DiagnosticFlowContext;
  onNavigate?: (page: "flow" | "history" | "analysis") => void;
  onViewHistoricalRun?: (jobId: string) => void;
  onLoadHistoricalFlow?: (flowLock: FlowLock, run: DiagnosticRunSummary) => void;
  onStartHistoricalRun?: (mode: "benchmark" | "diagnose", flowLock: FlowLock, run: DiagnosticRunSummary) => void;
}

type EvidenceSlot = "current" | "hermes" | "baseline";
type EvidenceCollection = Record<EvidenceSlot, ProfileEvidence | undefined>;
type AsyncRequest = "current" | "hermes" | "baseline" | "source-map" | "diff" | "managed";

const EMPTY_EVIDENCE: EvidenceCollection = { current: undefined, hermes: undefined, baseline: undefined };
const EMPTY_SOURCE_MAP: SourceMapEvidence = { state: "not-collected", mappedCount: 0 };
const RUN_PAGE_SIZE = 20;
const ASYNC_REQUESTS: readonly AsyncRequest[] = ["current", "hermes", "baseline", "source-map", "diff", "managed"];

function evidenceKind(slot: EvidenceSlot): ProfileEvidenceKind {
  return slot === "current" ? "react" : slot;
}

export function DiagnosticCenter(props: DiagnosticCenterProps) {
  return <DiagnosticErrorBoundary><DiagnosticCenterContent {...props} /></DiagnosticErrorBoundary>;
}

class DiagnosticErrorBoundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  state = { failed: false };

  static getDerivedStateFromError() {
    return { failed: true };
  }

  render() {
    if (this.state.failed) {
      return <div className="diagnostic-empty card"><AlertTriangle size={26} /><h2>性能诊断页面遇到异常</h2><p>Reactor 已阻止整页黑屏。请重试；若仍失败，可在设置中生成本地诊断包。</p><button className="secondary-button" onClick={() => this.setState({ failed: false })}><RefreshCw size={14} />重新加载诊断页</button></div>;
    }
    return this.props.children;
  }
}

function DiagnosticCenterContent(props: DiagnosticCenterProps) {
  const [runs, setRuns] = useState<DiagnosticRunSummary[]>([]);
  const [runsTotal, setRunsTotal] = useState(0);
  const [runsLoading, setRunsLoading] = useState(true);
  const [runsLoadingMore, setRunsLoadingMore] = useState(false);
  const [selectedFlowHash, setSelectedFlowHash] = useState<string>();
  const [selectedRunIdentity, setSelectedRunIdentity] = useState<string>();
  const [selectedFramework, setSelectedFramework] = useState<DiagnosticFramework>(props.activeFlow?.framework ?? "react-native");
  const [error, setError] = useState("");
  const loadToken = useRef(0);

  async function loadRuns(offset: number) {
    const token = ++loadToken.current;
    offset === 0 ? setRunsLoading(true) : setRunsLoadingMore(true);
    setError("");
    try {
      const page = await listDiagnosticRuns({ limit: RUN_PAGE_SIZE, offset });
      if (loadToken.current !== token) return;
      setRuns((current) => offset === 0 ? page.runs : mergeDiagnosticRuns(current, page.runs));
      setRunsTotal(page.total);
    } catch (reason) {
      if (loadToken.current === token) setError(`读取历史诊断 Run 失败：${String(reason)}`);
    } finally {
      if (loadToken.current === token) {
        setRunsLoading(false);
        setRunsLoadingMore(false);
      }
    }
  }

  useEffect(() => {
    void loadRuns(0);
    return () => { loadToken.current += 1; };
  }, []);

  useEffect(() => {
    const preferred = preferredDiagnosticFlowHash(runs, props.activeFlow?.flowHash);
    setSelectedFlowHash((current) => current && runs.some((run) => run.flowHash === current) ? current : preferred);
  }, [runs, props.activeFlow?.flowHash]);

  const selectedRun = preferredDiagnosticRun(runs, selectedFlowHash, selectedRunIdentity);
  useEffect(() => {
    setSelectedRunIdentity(selectedRun ? diagnosticRunIdentity(selectedRun) : undefined);
  }, [selectedRun?.jobId, selectedRun?.runId]);

  useEffect(() => {
    const next = normalizeFramework(selectedRun?.framework ?? props.activeFlow?.framework ?? "react-native");
    if (next) setSelectedFramework(next);
  }, [selectedRun?.runId, selectedRun?.framework, props.activeFlow?.framework]);

  const groups = useMemo(() => groupDiagnosticRunsByFlow(runs), [runs]);
  const framework = selectedFramework;
  const key = diagnosticWorkbenchKey(selectedRun?.jobId, selectedRun?.runId, selectedRun?.flowHash ?? selectedFlowHash, framework);

  return <>
    <header className="topbar diagnostic-topbar">
      <div><p className="eyebrow">PERFORMANCE DIAGNOSTICS</p><h1>性能诊断</h1></div>
      <div className="top-actions"><span className="status-pill ready"><span className="status-dot" />本地解析 · 不上传源码</span></div>
    </header>
    {error && <div className="error-banner">{error}</div>}
    <HistoricalRunSelector
      activeFlowHash={props.activeFlow?.flowHash}
      groups={groups}
      selectedFlowHash={selectedFlowHash}
      selectedRun={selectedRun}
      loading={runsLoading}
      loadingMore={runsLoadingMore}
      hasMore={runs.length < runsTotal}
      onFlow={(flowHash) => { setSelectedFlowHash(flowHash); setSelectedRunIdentity(undefined); }}
      onRun={setSelectedRunIdentity}
      onLoadMore={() => void loadRuns(runs.length)}
    />
    <DiagnosticWorkbench key={key} {...props} selectedRun={selectedRun} runsLoading={runsLoading} framework={framework} onFrameworkChange={setSelectedFramework} />
  </>;
}

function mergeDiagnosticRuns(current: DiagnosticRunSummary[], incoming: DiagnosticRunSummary[]) {
  const byIdentity = new Map(current.map((run) => [`${run.jobId}:${run.runId}`, run]));
  for (const run of incoming) byIdentity.set(`${run.jobId}:${run.runId}`, run);
  return [...byIdentity.values()];
}

function DiagnosticWorkbench({ activeFlow, onNavigate, onViewHistoricalRun, onLoadHistoricalFlow, onStartHistoricalRun, selectedRun, runsLoading, framework, onFrameworkChange }: DiagnosticCenterProps & { selectedRun?: DiagnosticRunSummary; runsLoading: boolean; framework: DiagnosticFramework; onFrameworkChange: (framework: DiagnosticFramework) => void }) {
  const [evidence, setEvidence] = useState<EvidenceCollection>(EMPTY_EVIDENCE);
  const [sourceMap, setSourceMap] = useState<SourceMapEvidence>(EMPTY_SOURCE_MAP);
  const [diff, setDiff] = useState<ProfileDiffReport>();
  const [view, setView] = useState<DiagnosticView>("overview");
  const [selectedCommit, setSelectedCommit] = useState<ProfileCommit>();
  const [rerunEligibility, setRerunEligibility] = useState<DiagnosticRerunEligibility>();
  const [historicalLock, setHistoricalLock] = useState<FlowLock | null>();
  const [actionLoading, setActionLoading] = useState(false);
  const [error, setError] = useState("");
  const requests = useRef(new RequestTokens(ASYNC_REQUESTS));

  const currentEvidence = evidence.current;
  const hermesEvidence = evidence.hermes;
  const baselineEvidence = evidence.baseline;
  const current = currentEvidence?.report;
  const hermes = hermesEvidence?.report;
  const baseline = baselineEvidence?.report;

  function startRequest(request: AsyncRequest) {
    return requests.current.start(request);
  }

  function requestIsCurrent(request: AsyncRequest, token: number) {
    return requests.current.isCurrent(request, token);
  }

  function setEvidenceSlot(slot: EvidenceSlot, value: ProfileEvidence | undefined) {
    setEvidence((previous) => ({ ...previous, [slot]: value }));
  }

  useEffect(() => () => {
    requests.current.cancelAll();
  }, []);

  useEffect(() => {
    if (!baseline || !current) {
      setDiff(undefined);
      return;
    }
    const token = startRequest("diff");
    setDiff(undefined);
    void diffProfileReports(baseline, current)
      .then((report) => {
        if (requestIsCurrent("diff", token)) setDiff(report);
      })
      .catch((reason) => {
        if (requestIsCurrent("diff", token)) setError(`Profile 统计差异计算失败：${String(reason)}`);
      });
    return () => { requests.current.cancel("diff"); };
  }, [baseline, current]);

  useEffect(() => {
    let cancelled = false;
    setRerunEligibility(undefined);
    setHistoricalLock(undefined);
    if (!selectedRun) return;
    void Promise.all([
      getDiagnosticRerunEligibility(selectedRun.jobId, selectedRun.runId, selectedRun.flowHash),
      loadHistoricalFlowLock(selectedRun.jobId, selectedRun.runId, selectedRun.flowHash),
    ]).then(([eligibility, lock]) => {
      if (cancelled) return;
      setRerunEligibility(eligibility);
      setHistoricalLock(lock);
    }).catch((reason) => {
      if (!cancelled) setError(`读取历史 Flow 重跑能力失败：${String(reason)}`);
    });
    return () => { cancelled = true; };
  }, [selectedRun?.jobId, selectedRun?.runId, selectedRun?.flowHash]);

  async function historicalAction(mode: "load" | "benchmark" | "diagnose") {
    if (!selectedRun) return;
    setActionLoading(true);
    setError("");
    try {
      const lock = historicalLock ?? await loadHistoricalFlowLock(selectedRun.jobId, selectedRun.runId, selectedRun.flowHash);
      setHistoricalLock(lock);
      if (!lock) throw new Error(rerunEligibility?.reason ?? "历史 Flow Lock 缺失，证据仍可分析，但不能重新运行");
      if (mode === "load") onLoadHistoricalFlow?.(lock, selectedRun);
      else {
        const blockedReferences = historicalRerunBlockingReferences(lock.flow);
        if (blockedReferences.length) {
          throw new Error(`历史重跑已阻止：Flow 包含 ${blockedReferences.join("、")}，当前入口不能安全重新确认输入，也不会读取旧 Prompt 或系统凭据库 Secret/TOTP`);
        }
        if (!rerunEligibility?.eligible) throw new Error(rerunEligibility?.reason ?? "后端未确认该历史 Flow 可安全重跑");
        onStartHistoricalRun?.(mode, lock, selectedRun);
      }
    } catch (reason) {
      setError(String(reason));
    } finally {
      setActionLoading(false);
    }
  }

  async function loadProfile(slot: EvidenceSlot, file?: File) {
    if (!file) return;
    if (slot === "current") startRequest("managed");
    startRequest("source-map");
    const token = startRequest(slot);
    setError("");
    setEvidenceSlot(slot, {
      kind: evidenceKind(slot),
      source: "local-file",
      state: "loading",
      fileName: file.name,
      producer: "用户导入",
      sameRunVerified: false,
    });
    try {
      const json = await file.text();
      if (!requestIsCurrent(slot, token)) return;
      const report = await analyzeProfileJson(json, sourceMap.json || undefined);
      if (!requestIsCurrent(slot, token)) return;
      if (slot === "current" && report.profileType !== "react_profiler") throw new Error("这里需要 React DevTools Profiler JSON；Hermes CPU Profile 请导入到独立的 JS 热点栏");
      if (slot === "hermes" && report.profileType !== "hermes_cpu") throw new Error("这里需要 Hermes / Chrome CPU Profile；React Profile 请导入到 Render 栏");
      if (slot === "baseline" && report.profileType !== "react_profiler") throw new Error("Profile 统计差异的基线需要 React DevTools Profiler JSON");
      setEvidenceSlot(slot, {
        kind: evidenceKind(slot),
        source: "local-file",
        state: "unverified",
        report,
        json,
        fileName: file.name,
        collector: report.sourceFormat,
        producer: profileTypeLabel(report),
        producerVersion: `schema-v${report.schemaVersion}`,
        sameRunVerified: false,
      });
      if (slot === "current") {
        setSelectedCommit(undefined);
        setView("overview");
      } else if (slot === "hermes") {
        setView("hermes");
      }
    } catch (reason) {
      if (!requestIsCurrent(slot, token)) return;
      const message = `无法导入 ${file.name}：${String(reason)}`;
      setEvidenceSlot(slot, {
        kind: evidenceKind(slot),
        source: "local-file",
        state: "error",
        fileName: file.name,
        producer: "用户导入",
        sameRunVerified: false,
        error: message,
      });
      setError(message);
    }
  }

  async function loadSourceMap(file?: File) {
    if (!file) return;
    const token = startRequest("source-map");
    setSourceMap({ state: "loading", fileName: file.name, mappedCount: 0 });
    setError("");
    try {
      const mapJson = await file.text();
      if (!requestIsCurrent("source-map", token)) return;
      const [nextCurrent, nextHermes, nextBaseline] = await Promise.all([
        currentEvidence?.json ? analyzeProfileJson(currentEvidence.json, mapJson) : Promise.resolve(undefined),
        hermesEvidence?.json ? analyzeProfileJson(hermesEvidence.json, mapJson) : Promise.resolve(undefined),
        baselineEvidence?.json ? analyzeProfileJson(baselineEvidence.json, mapJson) : Promise.resolve(undefined),
      ]);
      if (!requestIsCurrent("source-map", token)) return;
      if (nextCurrent && currentEvidence) setEvidenceSlot("current", { ...currentEvidence, report: nextCurrent });
      if (nextHermes && hermesEvidence) setEvidenceSlot("hermes", { ...hermesEvidence, report: nextHermes });
      if (nextBaseline && baselineEvidence) setEvidenceSlot("baseline", { ...baselineEvidence, report: nextBaseline });
      setSourceMap({
        state: "available",
        fileName: file.name,
        json: mapJson,
        mappedCount: (nextCurrent?.sourceMapMappedCount ?? current?.sourceMapMappedCount ?? 0) + (nextHermes?.sourceMapMappedCount ?? hermes?.sourceMapMappedCount ?? 0),
      });
    } catch (reason) {
      if (!requestIsCurrent("source-map", token)) return;
      const message = `无法应用 ${file.name}：${String(reason)}`;
      setSourceMap({ state: "error", fileName: file.name, mappedCount: 0, error: message });
      setError(message);
    }
  }

  function onFile(kind: "current" | "hermes" | "baseline", event: ChangeEvent<HTMLInputElement>) {
    void loadProfile(kind, event.target.files?.[0]);
    event.target.value = "";
  }

  function onSourceMap(event: ChangeEvent<HTMLInputElement>) {
    void loadSourceMap(event.target.files?.[0]);
    event.target.value = "";
  }

  const tabs = [
    ["overview", "性能总览", Activity],
    ["runtime", "受管运行时证据", Braces],
    ["render", "Render", Layers3],
    ["findings", "可疑渲染", AlertTriangle],
    ["hermes", "Hermes / JS CPU", Cpu],
    ["timeline", "时间线 / 火焰图", Timer],
    ["diff", "统计差异", GitCompare],
    ["source", "源码定位", MapPinned],
  ] as const;

  function drill(target: DiagnosticView, commitId?: string) {
    if (commitId && current) setSelectedCommit(current.commits.find((commit) => commit.id === commitId));
    setView(target);
  }

  const selectedResult = selectedRun ? { ...selectedRun.result, jobId: selectedRun.jobId } : undefined;
  const managedProfileArtifact = selectedResult ? findManagedReactProfileArtifact(selectedResult) : undefined;

  useEffect(() => {
    const profileFile = selectedResult?.androidNative?.rnDiagnostics?.profileFile;
    if (!profileFile || !managedProfileArtifact || currentEvidence?.source === "local-file") return;
    const token = startRequest("managed");
    setEvidenceSlot("current", {
      kind: "react",
      source: "managed-run",
      state: "loading",
      fileName: profileFile.split(/[\\/]/).pop() ?? "rn-profile.json",
      rawFile: profileFile,
      jobId: selectedResult.jobId,
      runId: selectedResult.runId,
      flowHash: selectedResult.flowHash,
      collector: selectedResult.androidNative?.rnDiagnostics?.collector,
      producer: "RN Profiling Renderer",
      sameRunVerified: false,
    });
    void analyzeManagedProfile(selectedResult.jobId!, selectedResult.runId, managedProfileArtifact)
      .then((report) => {
        if (!requestIsCurrent("managed", token)) return;
        setEvidenceSlot("current", {
          kind: "react",
          source: "managed-run",
          state: "available",
          report,
          fileName: profileFile.split(/[\\/]/).pop() ?? "rn-profile.json",
          rawFile: profileFile,
          jobId: selectedResult.jobId,
          runId: selectedResult.runId,
          flowHash: selectedResult.flowHash,
          collector: selectedResult.androidNative?.rnDiagnostics?.collector ?? report.sourceFormat,
          producer: "RN Profiling Renderer",
          producerVersion: `schema-v${report.schemaVersion}`,
          sameRunVerified: true,
        });
        setSelectedCommit(undefined);
      })
      .catch((reason) => {
        if (!requestIsCurrent("managed", token)) return;
        const message = `自动解析 Flow RN Profile 失败：${String(reason)}`;
        setEvidenceSlot("current", {
          kind: "react",
          source: "managed-run",
          state: "error",
          fileName: profileFile.split(/[\\/]/).pop() ?? "rn-profile.json",
          rawFile: profileFile,
          jobId: selectedResult.jobId,
          runId: selectedResult.runId,
          flowHash: selectedResult.flowHash,
          collector: selectedResult.androidNative?.rnDiagnostics?.collector,
          producer: "RN Profiling Renderer",
          sameRunVerified: false,
          error: message,
        });
        setError(message);
      });
    return () => { requests.current.cancel("managed"); };
  }, [selectedResult?.jobId, selectedResult?.runId, selectedResult?.androidNative?.rnDiagnostics?.profileFile, managedProfileArtifact?.path, managedProfileArtifact?.sizeBytes, managedProfileArtifact?.sha256]);

  return (
    <>
      {error && <div className="error-banner">{error}</div>}

      <HistoricalRunActions
        run={selectedRun}
        eligibility={rerunEligibility}
        lock={historicalLock}
        loading={actionLoading}
        onViewEvidence={onViewHistoricalRun && selectedRun ? () => onViewHistoricalRun(selectedRun.jobId) : undefined}
        onLoad={() => void historicalAction("load")}
        onBenchmark={() => void historicalAction("benchmark")}
        onDiagnose={() => void historicalAction("diagnose")}
        canLoad={Boolean(onLoadHistoricalFlow)}
        canRun={Boolean(onStartHistoricalRun)}
      />

      <section className="diagnostic-frameworks card" aria-label="框架选择">
        <div><b>选择框架</b><span>黑盒性能统一呈现，框架专项证据按能力接入</span></div>
        <div role="tablist">
          {(["react-native", "flutter", "lynx"] as DiagnosticFramework[]).map((item) => (
            <button key={item} className={framework === item ? "active" : ""} onClick={() => { onFrameworkChange(item); setView("overview"); }}>
              <span className={`framework-dot ${item === "react-native" ? "" : item}`} />{frameworkLabel(item)}
              {item !== "react-native" && <small>专项接入中</small>}
            </button>
          ))}
        </div>
      </section>

      <section className="diagnostic-workbench card">
        <div className="diagnostic-tabs" role="tablist" aria-label="性能诊断视图">
          {tabs.map(([id, label, Icon]) => (
            <button key={id} className={view === id ? "active" : ""} onClick={() => setView(id)} role="tab" aria-selected={view === id}>
              <Icon size={14} />{label}
              {id === "findings" && current && <b>{current.findings.length}</b>}
              {id === "diff" && diff && <b>{diff.regressionCount}</b>}
            </button>
          ))}
        </div>
        {view === "overview" && <PerformanceOverview framework={framework} result={selectedResult} currentEvidence={currentEvidence} hermesEvidence={hermesEvidence} loading={runsLoading} onDrill={drill} onNavigate={onNavigate} />}
        {framework !== "react-native" && view !== "overview" && <FrameworkPending framework={framework} onOverview={() => setView("overview")} />}
        {framework === "react-native" && view === "runtime" && <RuntimeDiagnostics result={selectedResult} onCollect={onNavigate ? () => onNavigate("flow") : undefined} />}
        {framework === "react-native" && view === "render" && (current ? <><EvidenceDetails evidence={currentEvidence} result={selectedResult} /><ComponentTable components={current.components} selectedCommit={selectedCommit} /></> : <EvidenceEmpty title="尚无 React Render Profile" detail="同 Flow 的受管 Run 未采集 Profile，或尚未导入本地 React Profiler JSON。本地文件只作为未验证的导入上下文。" cta="前往采集或导入" targetId="rn-profile-import" />)}
        {framework === "react-native" && view === "findings" && (current ? <><EvidenceDetails evidence={currentEvidence} result={selectedResult} /><Findings report={current} onDrill={(commitId) => drill(commitId ? "timeline" : "render", commitId)} /></> : <EvidenceEmpty title="可疑渲染规则需要 React Profile" detail="规则只检查 Profile 中已记录的变化字段与组件/Commit 关系；没有证据时不会产生结论。" cta="前往采集或导入" targetId="rn-profile-import" />)}
        {framework === "react-native" && view === "hermes" && (hermes ? <><EvidenceDetails evidence={hermesEvidence} /><FunctionHotspots report={hermes} /></> : <EvidenceEmpty title="尚无 Hermes / JS CPU Profile" detail="当前受管 Run 未提供 CPU Profile。可导入本地 Hermes 或 Chrome CPU Profile；其 Flow 身份保持未验证。" cta="导入 CPU Profile" targetId="rn-profile-import" />)}
        {framework === "react-native" && view === "timeline" && <><UnifiedTimeline jobId={selectedResult?.jobId} runId={selectedResult?.runId} />{current && <details className="timeline-profile-fallback"><summary>查看独立 React Profile 时间线与火焰图</summary><EvidenceDetails evidence={currentEvidence} result={selectedResult} /><CommitTimeline commits={current.commits} selected={selectedCommit} components={current.components} onSelect={setSelectedCommit} onInspectComponents={() => setView("render")} /><ComponentFlame components={current.components} /></details>}</>}
        {framework === "react-native" && view === "diff" && (diff ? <ProfileDiff diff={diff} baselineEvidence={baselineEvidence} currentEvidence={currentEvidence} /> : <EvidenceEmpty title="统计差异需要两份 React Profile" detail="导入当前与基线 Profile 后比较 Render 次数和累计耗时。除非来源兼容性可验证，否则只展示未验证统计差异。" cta="选择当前与基线" targetId="rn-profile-import" />)}
        {framework === "react-native" && view === "source" && <SourceMapPanel sourceMap={sourceMap} loading={sourceMap.state === "loading"} onChange={onSourceMap} />}
      </section>

      {framework === "react-native" && <section id="rn-profile-import" className="diagnostic-import card">
        <div className="card-heading">
          <div className="heading-icon purple"><Braces size={19} /></div>
          <div>
            <h2>采集或导入 RN 诊断证据</h2>
            <p>{selectedRun ? `受管证据必须来自 Job ${selectedRun.jobId.slice(0, 10)}… / Run ${selectedRun.runId.slice(0, 10)}… / Flow ${selectedRun.flowHash.slice(0, 12)}…；本地导入不会验证或绑定身份` : "选择历史 Run 后可加载同 Run 受管证据；本地 Profile 始终标记为未验证导入上下文"}</p>
          </div>
          <span className="schema-badge">PROFILE EVIDENCE v1</span>
        </div>
        <div className="diagnostic-files">
          <ProfileFilePicker label="React Render Profile" evidence={currentEvidence} loading={currentEvidence?.state === "loading"} required onChange={(event) => onFile("current", event)} />
          <ProfileFilePicker label="Hermes / JS CPU Profile" evidence={hermesEvidence} loading={hermesEvidence?.state === "loading"} onChange={(event) => onFile("hermes", event)} />
          <ProfileFilePicker label="React 基线（可选）" evidence={baselineEvidence} loading={baselineEvidence?.state === "loading"} onChange={(event) => onFile("baseline", event)} />
        </div>
        <div className={`diagnostic-source-map ${sourceMap.state === "available" ? "loaded" : sourceMap.state === "error" ? "failed" : ""}`}>
          <div>
            <span>Source Map（可选）</span>
            <b>{sourceMap.fileName || "将 bundle.js 位置映射回 TypeScript / TSX 源码"}</b>
            <small>{sourceMapStatus(sourceMap)}</small>
          </div>
          <label className="secondary-button diagnostic-upload">
            {sourceMap.state === "loading" ? <RefreshCw size={14} className="spin" /> : sourceMap.state === "available" ? <Check size={14} /> : <Upload size={14} />}
            {sourceMap.state === "available" ? "更换 Map" : "选择 .map"}
            <input type="file" accept=".json,.map" onChange={onSourceMap} disabled={sourceMap.state === "loading"} />
          </label>
        </div>
        <p className="diagnostic-privacy">Profile、Source Location 和调用栈只在本机 Rust 核心中解析；本页不会调用 AI。手工文件不具备 Run/Flow 同源证明。</p>
      </section>}
    </>
  );
}

function HistoricalRunSelector({
  activeFlowHash,
  groups,
  selectedFlowHash,
  selectedRun,
  loading,
  loadingMore,
  hasMore,
  onFlow,
  onRun,
  onLoadMore,
}: {
  activeFlowHash?: string;
  groups: ReturnType<typeof groupDiagnosticRunsByFlow>;
  selectedFlowHash?: string;
  selectedRun?: DiagnosticRunSummary;
  loading: boolean;
  loadingMore: boolean;
  hasMore: boolean;
  onFlow: (flowHash: string) => void;
  onRun: (runId: string) => void;
  onLoadMore: () => void;
}) {
  const runs = groups.find((group) => group.flowHash === selectedFlowHash)?.runs ?? [];
  return <section className="diagnostic-history-selector card">
    <div className="diagnostic-selector-heading"><div><b>历史 Flow / Run</b><span>Flow 按 flowHash 分组；默认优先当前 Flow Studio 的锁定 Flow。</span></div>{loading && <RefreshCw size={15} className="spin" />}</div>
    {groups.length ? <div className="diagnostic-selector-grid">
      <label><span>Flow</span><select value={selectedFlowHash ?? ""} onChange={(event) => onFlow(event.target.value)}>{groups.map((group) => {
        const latest = group.runs[0];
        return <option key={group.flowHash} value={group.flowHash}>{group.flowHash === activeFlowHash ? "当前 · " : ""}{latest.flowName ?? latest.appId ?? "未命名 Flow"} · {group.flowHash.slice(0, 12)}… ({group.runs.length})</option>;
      })}</select></label>
      <label><span>Run</span><select value={selectedRun ? diagnosticRunIdentity(selectedRun) : ""} onChange={(event) => onRun(event.target.value)}>{runs.map((run) => <option key={diagnosticRunIdentity(run)} value={diagnosticRunIdentity(run)}>{new Date(run.createdAt).toLocaleString()} · {run.platform} · {run.runId.slice(0, 10)}…</option>)}</select></label>
    </div> : !loading && <div className="diagnostic-run-empty"><Activity size={20} /><div><b>没有可分析的历史 Run</b><span>列表只包含后端返回的可用诊断结果。</span></div></div>}
    {selectedRun && <div className="diagnostic-run-badges"><span>{frameworkLabel(normalizeFramework(selectedRun.framework) ?? "react-native")}</span><span>{selectedRun.platform}</span><span>{selectedRun.devicePhysical ? "物理设备" : "模拟器"}</span><span>{selectedRun.successfulIterationCount}/{selectedRun.iterationCount} 成功</span><span className={selectedRun.lockAvailable ? "verified" : "warning"}>{selectedRun.lockAvailable ? "Flow Lock 可用" : "Flow Lock 缺失"}</span>{selectedRun.synthetic && <span className="warning">Synthetic</span>}</div>}
    {hasMore && <button className="secondary-button diagnostic-load-more" disabled={loadingMore} onClick={onLoadMore}>{loadingMore && <RefreshCw size={14} className="spin" />}加载更多 Run</button>}
  </section>;
}

function HistoricalRunActions({ run, eligibility, lock, loading, onViewEvidence, onLoad, onBenchmark, onDiagnose, canLoad, canRun }: {
  run?: DiagnosticRunSummary;
  eligibility?: DiagnosticRerunEligibility;
  lock?: FlowLock | null;
  loading: boolean;
  onViewEvidence?: () => void;
  onLoad: () => void;
  onBenchmark: () => void;
  onDiagnose: () => void;
  canLoad: boolean;
  canRun: boolean;
}) {
  if (!run) return <section className="diagnostic-flow-context card pending"><div><span>历史工作台</span><b>请选择 Flow 和 Run</b><small>证据分析不要求 Flow Lock；重新运行要求后端验证历史锁。</small></div></section>;
  const reason = eligibility?.reason ?? (!run.lockAvailable || lock === null ? "历史 Flow Lock 缺失：仍可分析证据，但不能重新运行" : "正在验证重跑条件");
  const exactEligibility = eligibility?.jobId === run.jobId && eligibility.runId === run.runId;
  const rerunnable = Boolean(exactEligibility && eligibility?.eligible && eligibility.lockAvailable && lock);
  const diagnoseAvailable = rerunnable && eligibility?.platform === "android" && eligibility.diagnoseAvailable;
  return <section className={`diagnostic-flow-context card ${run.lockAvailable ? "linked" : "pending"}`}>
    <div><span>所选历史 Run</span><b>{run.flowName ?? run.appId ?? run.flowHash}</b><small>{run.jobId} · {run.runId} · {run.flowHash} · {run.framework}</small>{!rerunnable && <small className="diagnostic-disabled-reason">{reason}</small>}</div>
    <div className="diagnostic-history-actions">
      {onViewEvidence && <button className="secondary-button" onClick={onViewEvidence}>查看原始证据</button>}
      <button className="secondary-button" disabled={!canLoad || !lock || loading} onClick={onLoad} title={reason}>加载验证 Flow</button>
      <button className="secondary-button" disabled={!canRun || !rerunnable || loading} onClick={onBenchmark} title={reason}>新 Benchmark</button>
      <button className="primary-button" disabled={!canRun || !diagnoseAvailable || loading} onClick={onDiagnose} title={run.platform === "ios" ? "iOS Diagnose 暂不可用" : reason}>{run.platform === "ios" ? "iOS Diagnose 不可用" : "新 Diagnose"}</button>
      <small>加载只切换到 Flow Studio，不自动运行。新运行不会复用旧设备、Secret 或 Prompt 输入。</small>
    </div>
  </section>;
}

function ProfileFilePicker({
  label,
  evidence,
  loading,
  required,
  onChange,
}: {
  label: string;
  evidence?: ProfileEvidence;
  loading: boolean;
  required?: boolean;
  onChange: (event: ChangeEvent<HTMLInputElement>) => void;
}) {
  const report = evidence?.report;
  return (
    <div className={`diagnostic-file ${report ? "loaded" : ""} ${evidence?.state === "error" ? "failed" : ""}`}>
      <div>
        <span>{label}</span>
        <b>{evidence?.fileName || (required ? "尚未选择" : label.includes("Hermes") ? "尚未导入 JS CPU 证据" : "用于检查次数和耗时统计差异")}</b>
        {evidence && <small className={evidence.sameRunVerified ? "verified" : "unverified"}>{evidenceStateLabel(evidence)}</small>}
        {report && <small>{profileTypeLabel(report)} · {report.profileId.slice(-8)}</small>}
      </div>
      <label className="secondary-button diagnostic-upload">
        {loading ? <RefreshCw size={14} className="spin" /> : report ? <Check size={14} /> : <Upload size={14} />}
        {report ? "更换" : "选择 JSON"}
        <input type="file" accept=".json,.cpuprofile" onChange={onChange} disabled={loading} />
      </label>
    </div>
  );
}

function PerformanceOverview({
  framework,
  result,
  currentEvidence,
  hermesEvidence,
  loading,
  onDrill,
  onNavigate,
}: {
  framework: DiagnosticFramework;
  result?: NormalizedResult;
  currentEvidence?: ProfileEvidence;
  hermesEvidence?: ProfileEvidence;
  loading: boolean;
  onDrill: (target: DiagnosticView) => void;
  onNavigate?: (page: "flow" | "history" | "analysis") => void;
}) {
  const current = currentEvidence?.report;
  const hermes = hermesEvidence?.report;
  const frameP95 = firstFiniteMetric(result?.androidNative?.frameTimeP95Ms, result?.iosNative?.frameTimeP95Ms);
  const cpu = firstFiniteMetric(result?.summary.cpuMeanPct, result?.iosNative?.cpuMeanPct);
  const memory = result?.platform === "android"
    ? firstFiniteMetric(result.androidNative?.memoryPssMb)
    : firstFiniteMetric(result?.iosNative?.memoryPeakMb, result?.summary.ramPeakMb);
  const startup = firstFiniteMetric(result?.androidNative?.startupTimeMs, result?.iosNative?.startupTimeMs);
  const memoryLabel = result?.platform === "android" ? "测后 PSS" : "内存峰值";
  const metrics: Array<[string, number | undefined, string, DiagnosticView, string]> = [
    ["P95 帧耗时", frameP95, "ms", "timeline", "查看独立的 Commit 证据"],
    ["Jank", firstFiniteMetric(result?.androidNative?.jankFramePct), "%", "timeline", "查看独立的 Commit 证据"],
    ["CPU", cpu, "%", "hermes", "查看独立的 JS CPU 证据"],
    [memoryLabel, memory, "MB", "runtime", "查看受管运行时证据"],
    ["冷启动", startup, "ms", "render", "查看独立的首次挂载证据"],
  ];
  return (
    <div className="diagnostic-panel performance-overview">
      <div className="diagnostic-panel-heading">
        <div><h2>{frameworkLabel(framework)} 性能总览</h2><p>搜索范围内的可用黑盒 Run 与框架专项 Profile 并列展示；并列不表示因果。</p></div>
        {result && <span>{result.platform} · {result.device?.physical ? "物理设备" : "模拟器"} · {result.summary.successfulIterationCount} 次成功</span>}
      </div>
      {loading ? <div className="skeleton-list" /> : result ? (
        <div className="diagnostic-overview-metrics">
          {metrics.map(([label, value, unit, target, hint]) => (
            <button key={label} onClick={() => onDrill(target)} disabled={framework !== "react-native"}>
              <span>{label}</span><strong>{value === undefined ? "—" : value.toFixed(value >= 100 ? 0 : 1)}</strong><small>{value === undefined ? "当前平台不可用" : unit}</small>
              <i>{framework === "react-native" ? hint : "专项下钻接入中"}<ChevronRight size={13} /></i>
            </button>
          ))}
        </div>
      ) : (
        <div className="diagnostic-run-empty"><Activity size={21} /><div><b>搜索范围内没有 {frameworkLabel(framework)} 的可用 Run</b><span>可用 Run 必须已完成、有结果、非 synthetic，且成功迭代数大于 0。请运行当前 Flow 或到历史记录确认结果。</span></div>{onNavigate && <button className="secondary-button" onClick={() => onNavigate("flow")}>前往运行 Flow</button>}</div>
      )}
      {framework === "react-native" ? (
        <div className="rn-evidence-status">
          <button className={current ? "ready" : ""} onClick={() => onDrill("render")}><Layers3 size={17} /><div><b>React Render</b><span>{current ? `${current.components.length} 个组件 · ${current.commitCount} commits · ${evidenceShortSource(currentEvidence)}` : "未采集或未导入"}</span></div><ChevronRight size={15} /></button>
          <button className={current?.findings.length ? "warning" : current ? "ready" : ""} onClick={() => onDrill("findings")}><AlertTriangle size={17} /><div><b>可疑渲染</b><span>{current ? `${current.findings.length} 项规则命中` : "需要 React Profile"}</span></div><ChevronRight size={15} /></button>
          <button className={hermes ? "ready" : ""} onClick={() => onDrill("hermes")}><Cpu size={17} /><div><b>Hermes / JS CPU</b><span>{hermes ? `${hermes.functions.length} 个函数热点 · ${evidenceShortSource(hermesEvidence)}` : "未采集或未导入"}</span></div><ChevronRight size={15} /></button>
          <button className={current?.sourceMapApplied || hermes?.sourceMapApplied ? "ready" : ""} onClick={() => onDrill("source")}><MapPinned size={17} /><div><b>源码定位</b><span>{current?.sourceMapApplied || hermes?.sourceMapApplied ? "Source Map 已应用" : "尚无成功映射"}</span></div><ChevronRight size={15} /></button>
        </div>
      ) : <FrameworkRoadmap framework={framework} />}
      {result && onNavigate && <div className="diagnostic-overview-actions"><button className="secondary-button" onClick={() => onNavigate("history")}>查看原始运行</button><button className="secondary-button" onClick={() => onNavigate("analysis")}>进行回归对比</button></div>}
    </div>
  );
}

function RuntimeDiagnostics({ result, onCollect }: { result?: NormalizedResult; onCollect?: () => void }) {
  const evidence = result?.androidNative?.rnDiagnostics;
  if (!result) return <EvidenceEmpty title="没有可用 Run，无法展示受管运行时证据" detail="受管运行时证据只能来自已完成、非 synthetic 且至少有一次成功迭代的 Run，不能通过导入 Profile 代替。" cta="返回 Flow 运行采集" onAction={onCollect} />;
  if (!evidence) return <EvidenceEmpty title="当前 Run 未采集 RN 受管运行时证据" detail="目标 App 需要接入 Reactor RN SDK 并执行诊断采集；正式 Benchmark 仍有效，但不会生成组件树、Console、Network 或对象保留证据。" cta="返回 Flow 配置采集" onAction={onCollect} />;
  const recentEvents = evidence.recentEvents ?? [];
  const latestTreeEvent = [...recentEvents].reverse().find((event) => event.kind === "component_tree");
  const treeNodes = Array.isArray(latestTreeEvent?.payload.nodes) ? latestTreeEvent.payload.nodes as Array<Record<string, unknown>> : [];
  const timeline = recentEvents.filter((event) => ["component_render", "component_tree", "react_profile", "console", "network", "object_lifecycle", "hermes_heap"].includes(event.kind)).slice(-24);
  return (
    <div className="diagnostic-panel runtime-diagnostics">
      <EvidenceDetails result={result} runtimeCollector={evidence.collector} rawFile={evidence.eventFile} />
      <div className="diagnostic-panel-heading"><div><h2>Flow 自动绑定的 RN 受管运行时证据</h2><p>{evidence.collector} · {evidence.benchmarkMode ?? "未声明模式"} · {evidence.eventCount} 条本地事件</p></div><span>{evidence.profileCommitCount ? "Profiling Renderer" : "生产 Renderer · 无耗时 Profile"}</span></div>
      <div className="runtime-diagnostic-facts">
        <div><span>组件树 Commit</span><b>{evidence.componentTreeCommitCount ?? 0}</b></div>
        <div><span>Profiler Commit</span><b>{evidence.profileCommitCount}</b></div>
        <div><span>Console</span><b>{evidence.consoleEventCount}</b></div>
        <div><span>Network</span><b>{evidence.networkEventCount}</b></div>
        <div><span>Hermes Heap</span><b>{evidence.hermesHeapSampleCount ?? 0}</b></div>
        <div><span>JS 堆快照</span><b>{evidence.hermesHeapSnapshotFile ? "已保存" : "未采集"}</b></div>
        <div><span>Java HPROF</span><b>{evidence.javaHeapDumpFile ? "已保存" : "未采集"}</b></div>
        <div><span>保留对象</span><b>{evidence.retainedObjectCount}</b></div>
        <div><span>显式保留</span><b>{formatDiagnosticBytes(evidence.retainedBytes)}</b></div>
      </div>
      <div className="runtime-component-tree"><h3>最新 React 组件树</h3>{treeNodes.length ? treeNodes.map((node, index) => <div key={String(node.id ?? index)} style={{ paddingLeft: `${Math.min(Number(node.depth ?? 0), 8) * 14}px` }}><Layers3 size={14} /><span>{String(node.name ?? "Anonymous")}</span></div>) : evidence.componentNames.map((name) => <div key={name}><Layers3 size={14} /><span>{name}</span></div>)}</div>
      <div className="runtime-event-timeline">
        <h3>Flow 期间事件时间线</h3>
        {timeline.length ? timeline.map((event, index) => <div key={`${event.timestampMs}-${event.kind}-${index}`}><time>{new Date(event.timestampMs).toLocaleTimeString()}</time><b>{runtimeEventLabel(event.kind)}</b><span>{runtimeEventSummary(event.kind, event.payload)}</span></div>) : <p>当前 Run 没有可展示的详细事件。</p>}
      </div>
      <div className="diagnostic-note">Console 与 Network 只保存脱敏的本地事件；查询参数、Header 和 Body 不进入证据。JS/Java 堆快照只在独立诊断构建中于测量后生成，不混入正式 Release Benchmark 数值。</div>
    </div>
  );
}

function runtimeEventLabel(kind: string) {
  return ({ component_render: "Render", component_tree: "Tree", react_profile: "Commit", console: "Console", network: "Network", object_lifecycle: "Object", hermes_heap: "Hermes Heap" } as Record<string, string>)[kind] ?? kind;
}

function runtimeEventSummary(kind: string, payload: Record<string, unknown>) {
  if (kind === "component_render") return `${String(payload.name ?? "Unknown")} · parent ${String(payload.parent ?? "—")}`;
  if (kind === "component_tree") return `Commit #${String(payload.commit ?? "—")} · ${String(payload.nodeCount ?? 0)} 个组件${payload.truncated ? " · 已截断" : ""}`;
  if (kind === "react_profile") return `${String(payload.id ?? "Unknown")} · ${formatMs(typeof payload.actualDuration === "number" ? payload.actualDuration : 0)} · ${String(payload.phase ?? "commit")}`;
  if (kind === "console") return `${String(payload.level ?? "log")} · ${Array.isArray(payload.values) ? payload.values.join(" ").slice(0, 240) : ""}`;
  if (kind === "network") return `${String(payload.method ?? "GET")} ${String(payload.url ?? "")} · ${String(payload.status ?? payload.event ?? "")}`;
  if (kind === "object_lifecycle") return `${String(payload.objectId ?? "object")} · ${String(payload.action ?? "event")} · ${formatDiagnosticBytes(typeof payload.bytes === "number" ? payload.bytes : 0)}`;
  if (kind === "hermes_heap") return `${String(payload.label ?? "sample")} · ${Object.keys((payload.stats as Record<string, unknown> | undefined) ?? {}).length} 项 VM 指标`;
  return kind;
}

function formatDiagnosticBytes(value: number) {
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KiB`;
  return `${(value / 1024 / 1024).toFixed(1)} MiB`;
}

function EvidenceEmpty({ title, detail, cta, targetId, onAction }: { title: string; detail: string; cta: string; targetId?: string; onAction?: () => void }) {
  const action = onAction ?? (targetId ? () => document.getElementById(targetId)?.scrollIntoView({ behavior: "smooth" }) : undefined);
  return <div className="diagnostic-empty inline"><Upload size={24} /><h2>{title}</h2><p>{detail}</p>{action && <button className="secondary-button" onClick={action}>{cta}</button>}</div>;
}

function FrameworkPending({ framework, onOverview }: { framework: DiagnosticFramework; onOverview: () => void }) {
  return <div className="diagnostic-empty inline"><Layers3 size={24} /><h2>{frameworkLabel(framework)} 专项诊断正在接入</h2><p>黑盒 FPS、帧耗时、CPU、内存和启动指标已经进入统一总览；当前不会用 RN 的组件语义冒充 {frameworkLabel(framework)} 专项证据。</p><button className="secondary-button" onClick={onOverview}>返回性能总览</button></div>;
}

function FrameworkRoadmap({ framework }: { framework: DiagnosticFramework }) {
  const items = framework === "flutter" ? ["Flutter DevTools Timeline", "Widget rebuild 统计", "Dart CPU / Allocation", "Shader / Raster jank"] : ["Lynx Trace / Timing", "组件更新次数", "JS / Native 双线程热点", "源码映射"];
  return <div className="framework-roadmap"><div><b>{frameworkLabel(framework)} 专项接入边界</b><span>以下能力登记在同一工作台，未采集前不生成占位结论。</span></div>{items.map((item) => <span key={item}>{item}<small>待接入</small></span>)}</div>;
}

function SourceMapPanel({ sourceMap, loading, onChange }: { sourceMap: SourceMapEvidence; loading: boolean; onChange: (event: ChangeEvent<HTMLInputElement>) => void }) {
  return <div className="diagnostic-panel"><div className="diagnostic-panel-heading"><div><h2>Source Map / 源码定位</h2><p>把 Hermes bundle 行列和组件位置映射回 TypeScript / TSX；加载成功不等于存在可映射位置。</p></div><span className={sourceMap.state === "available" && sourceMap.mappedCount === 0 ? "source-zero" : ""}>{sourceMapStatus(sourceMap)}</span></div><div className={`diagnostic-source-map source-panel ${sourceMap.state === "available" ? "loaded" : sourceMap.state === "error" ? "failed" : ""}`}><MapPinned size={20} /><div><span>当前 Source Map</span><b>{sourceMap.fileName || "尚未导入 .map 文件"}</b><small>{sourceMap.state === "available" ? sourceMap.mappedCount > 0 ? "已在本机重新解析所有本地导入 Profile" : "文件已成功读取，但当前 Profile 没有匹配的生成位置" : sourceMap.error ?? "不会上传源码或调用 AI"}</small></div><label className="secondary-button diagnostic-upload">{loading ? <RefreshCw size={14} className="spin" /> : sourceMap.state === "available" ? <Check size={14} /> : <Upload size={14} />}{sourceMap.state === "available" ? "更换 Map" : "选择 .map"}<input type="file" accept=".json,.map" onChange={onChange} disabled={loading} /></label></div></div>;
}

function findManagedReactProfileArtifact(result: NormalizedResult): DiagnosticArtifactRef | undefined {
  const declaredPath = result.androidNative?.rnDiagnostics?.profileFile;
  if (!declaredPath) return undefined;
  const declaredName = declaredPath.split(/[\\/]/).pop();
  return Object.values(result.frameworkDiagnostics?.reactNative?.collectors ?? {})
    .flatMap((collector) => collector.artifacts ?? [])
    .find((artifact) => artifact.integrity === "complete"
      && artifact.format === "react-devtools-profile-json"
      && (artifact.path === declaredPath || artifact.path.split(/[\\/]/).pop() === declaredName));
}

function normalizeFramework(value: string): DiagnosticFramework | undefined {
  const normalized = value.toLowerCase().replace(/[_\s]/g, "-");
  if (normalized === "react-native" || normalized === "reactnative" || normalized === "rn") return "react-native";
  if (normalized === "flutter") return "flutter";
  if (normalized === "lynx") return "lynx";
  return undefined;
}

function frameworkLabel(framework: DiagnosticFramework) {
  return framework === "react-native" ? "React Native" : framework === "flutter" ? "Flutter" : "Lynx";
}

function firstFiniteMetric(...values: unknown[]): number | undefined {
  return values.find((value): value is number => typeof value === "number" && Number.isFinite(value));
}

function ComponentTable({
  components,
  selectedCommit,
}: {
  components: ComponentProfileStat[];
  selectedCommit?: ProfileCommit;
}) {
  const visible = selectedCommit
    ? components.filter((component) => selectedCommit.renderedComponentIds.includes(component.id))
    : components;
  const maxCount = Math.max(...visible.map((component) => component.renderCount), 1);
  return (
    <div className="diagnostic-panel">
      <div className="diagnostic-panel-heading">
        <div>
          <h2>{selectedCommit ? `Commit ${selectedCommit.id} 的组件` : "组件 Render 热点"}</h2>
          <p>按 Render 次数排序；可在时间线选择 Commit 缩小范围。</p>
        </div>
        {selectedCommit && <span>{selectedCommit.renderedComponentIds.length} 个组件</span>}
      </div>
      <div className="component-table diagnostic-table">
        <div className="diagnostic-table-head"><span>组件 / 源码</span><span>Render</span><span>Commit</span><span>Total</span><span>Self</span><span>P95 / Max</span></div>
        {visible.slice(0, 100).map((component) => (
          <div className="diagnostic-table-row" key={component.id}>
            <div className="component-identity">
              <b>{component.name}</b>
              <small>{sourceLabel(component.source) || (component.parentName ? `父组件 ${component.parentName}` : "无源码位置")}</small>
              <small>{component.unchangedRenderCount} 次未记录变化字段 · {component.updaterCount} 次触发更新</small>
              <span style={{ width: `${Math.max(4, component.renderCount / maxCount * 100)}%` }} />
            </div>
            <strong>{component.renderCount}</strong>
            <span>{component.commitCount}</span>
            <span>{formatMs(component.totalTimeMs)}</span>
            <span>{formatMs(component.selfTimeMs)}</span>
            <span>{formatMs(component.p95TimeMs)} / {formatMs(component.maxTimeMs)}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

function CommitTimeline({
  commits,
  selected,
  components,
  onSelect,
  onInspectComponents,
}: {
  commits: ProfileCommit[];
  selected?: ProfileCommit;
  components: ComponentProfileStat[];
  onSelect: (commit: ProfileCommit) => void;
  onInspectComponents: () => void;
}) {
  const maxDuration = Math.max(...commits.map((commit) => commit.durationMs ?? 0), 1);
  const componentNames = new Map(components.map((component) => [component.id, component.name]));
  return (
    <div className="diagnostic-panel timeline-panel">
      <div className="diagnostic-panel-heading">
        <div><h2>Commit 时间线</h2><p>选择异常 Commit，下钻查看参与渲染的组件和变化证据。</p></div>
        <span>{commits.length} commits</span>
      </div>
      <div className="commit-timeline">
        {commits.map((commit) => (
          <button className={selected?.id === commit.id ? "active" : ""} key={commit.id} onClick={() => onSelect(commit)}>
            <span className="commit-index">#{commit.index + 1}</span>
            <span className="commit-track"><i style={{ width: `${Math.max(3, (commit.durationMs ?? 0) / maxDuration * 100)}%` }} /></span>
            <b>{formatMs(commit.durationMs ?? 0)}</b>
            <small>{commit.renderedComponentIds.length} renders · {commit.changedComponentIds.length} changed</small>
          </button>
        ))}
      </div>
      {selected && (
        <div className="commit-detail">
          <b>Commit {selected.id}</b>
          <span>{selected.timestampMs === undefined ? "无时间戳" : `${selected.timestampMs.toFixed(1)} ms`}</span>
          <p>{selected.renderedComponentIds.map((id) => componentNames.get(id) ?? `#${id}`).join(" · ")}</p>
          {selected.changes.length > 0 && (
            <ul>
              {selected.changes.map((change) => (
                <li key={change.componentId}>
                  <b>{componentNames.get(change.componentId) ?? `#${change.componentId}`}</b>
                  <span>{changeEvidenceLabel(change)}</span>
                </li>
              ))}
            </ul>
          )}
          {selected.updaterComponentIds.length > 0 && (
            <small>触发来源：{selected.updaterComponentIds.map((id) => componentNames.get(id) ?? `#${id}`).join(" · ")}</small>
          )}
          <button className="secondary-button commit-drill" onClick={onInspectComponents}>查看该 Commit 的组件</button>
        </div>
      )}
    </div>
  );
}

function ComponentFlame({ components }: { components: ComponentProfileStat[] }) {
  const byId = new Map(components.map((component) => [component.id, component]));
  const maxTime = Math.max(...components.map((component) => component.totalTimeMs), 1);
  function depth(component: ComponentProfileStat) {
    let value = 0;
    let parentId = component.parentId;
    const visited = new Set<string>();
    while (parentId && value < 8 && !visited.has(parentId)) {
      visited.add(parentId);
      value += 1;
      parentId = byId.get(parentId)?.parentId;
    }
    return value;
  }
  return (
    <div className="diagnostic-panel">
      <div className="diagnostic-panel-heading"><div><h2>组件火焰视图</h2><p>条形宽度表示累计耗时，缩进表示父子层级。</p></div></div>
      <div className="component-flame">
        {[...components].sort((a, b) => b.totalTimeMs - a.totalTimeMs).slice(0, 80).map((component) => (
          <div className="flame-row" key={component.id} style={{ paddingLeft: `${depth(component) * 18}px` }}>
            <div style={{ width: `${Math.max(8, component.totalTimeMs / maxTime * 100)}%` }}>
              <b>{component.name}</b><span>{formatMs(component.totalTimeMs)} total · {formatMs(component.selfTimeMs)} self</span>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

function Findings({ report, onDrill }: { report: DiagnosticProfileReport; onDrill: (commitId?: string) => void }) {
  return (
    <div className="diagnostic-panel">
      <div className="diagnostic-panel-heading"><div><h2>可疑渲染规则命中</h2><p>不调用 AI；规则只引用组件、Commit 和 Profile 已记录字段，不证明因果。</p></div></div>
      {report.findings.length === 0 ? (
        <div className="diagnostic-ok"><Check size={18} /><div><b>当前规则没有命中</b><span>这不代表不存在性能问题，也不代表所有 Props/State 都已记录。</span></div></div>
      ) : (
        <div className="finding-list">
          {report.findings.map((finding, index) => (
            <article className={`finding-card ${finding.severity}`} key={`${finding.ruleId}-${finding.componentId ?? index}`}>
              <AlertTriangle size={17} />
              <div>
                <div><b>{finding.title}</b><span>{finding.ruleId}</span></div>
                <p>{finding.summary}</p>
                {finding.source && <code>{sourceLabel(finding.source)}</code>}
                <small>{finding.commitIds.length ? `Commits: ${finding.commitIds.join(", ")}` : finding.evidenceRefs.join(" · ")}</small>
                <button className="finding-drill" onClick={() => onDrill(finding.commitIds[0])}>下钻到{finding.commitIds.length ? "异常 Commit" : "组件 Render"}<ChevronRight size={13} /></button>
              </div>
            </article>
          ))}
        </div>
      )}
      {report.warnings.map((warning) => <p className="diagnostic-warning" key={warning}>{warning}</p>)}
    </div>
  );
}

function FunctionHotspots({ report }: { report: DiagnosticProfileReport }) {
  const max = Math.max(...report.functions.map((fn) => fn.selfTimeMs), 1);
  return (
    <div className="diagnostic-panel">
      <div className="diagnostic-panel-heading"><div><h2>Hermes / JS CPU 热点</h2><p>按采样 Self Time 排序，用于定位 JavaScript 调用热点。</p></div></div>
      <div className="function-list">
        {report.functions.slice(0, 100).map((fn) => (
          <div key={fn.id}>
            <div><b>{fn.name}</b><span>{fn.selfTimePct.toFixed(1)}%</span></div>
            <i><span style={{ width: `${Math.max(2, fn.selfTimeMs / max * 100)}%` }} /></i>
            <small>{formatMs(fn.selfTimeMs)} · {fn.sampleCount} samples · {sourceLabel(fn.source) || "无源码位置"}</small>
          </div>
        ))}
      </div>
    </div>
  );
}

function ProfileDiff({
  diff,
  baselineEvidence,
  currentEvidence,
}: {
  diff: ProfileDiffReport;
  baselineEvidence?: ProfileEvidence;
  currentEvidence?: ProfileEvidence;
}) {
  const verifiedComparison = Boolean(
    diff.compatible
    && baselineEvidence?.sameRunVerified
    && currentEvidence?.sameRunVerified
    && baselineEvidence.flowHash
    && baselineEvidence.flowHash === currentEvidence.flowHash,
  );
  return (
    <div className="diagnostic-panel">
      <div className="diagnostic-panel-heading">
        <div><h2>{verifiedComparison ? "组件 Profile 对比" : "组件 Profile 未验证统计差异"}</h2><p>{baselineEvidence?.fileName ?? "基线"} → {currentEvidence?.fileName ?? "当前"}</p></div>
        <span className={verifiedComparison && diff.regressionCount ? "diff-regressed" : "diff-unverified"}>{verifiedComparison ? `${diff.regressionCount} 项规则回归` : `${diff.components.length} 项统计差异 · 未验证`}</span>
      </div>
      {!verifiedComparison && <div className="diagnostic-note">当前证据至少一份来自本地文件，或缺少同 Flow/兼容性证明。数值仅表示统计差异，不称为回归或稳定。</div>}
      {!diff.compatible && <div className="error-banner inline">解析器报告不兼容：{diff.reasons.join("；")}</div>}
      <div className="profile-diff-list">
        {diff.components.map((component) => (
          <div className={verifiedComparison && component.regressed ? "regressed" : ""} key={component.key}>
            <div><b>{component.name}</b><small>{sourceLabel(component.source) || component.key}</small></div>
            <span>Render <b>{component.baselineRenderCount} → {component.currentRenderCount}</b><small>{signed(component.renderCountDelta)} 次</small></span>
            <span>Total <b>{formatMs(component.baselineTotalTimeMs)} → {formatMs(component.currentTotalTimeMs)}</b><small>{signed(component.totalTimeDeltaPct, "%")}</small></span>
            <strong>{verifiedComparison ? component.newComponent ? "新增" : component.removedComponent ? "消失" : component.regressed ? "回归" : "规则内稳定" : component.newComponent ? "新增" : component.removedComponent ? "消失" : "差异"}</strong>
          </div>
        ))}
      </div>
    </div>
  );
}

function evidenceStateLabel(evidence: ProfileEvidence) {
  if (evidence.state === "loading") return "正在解析";
  if (evidence.state === "error") return evidence.error ?? "解析失败";
  if (evidence.sameRunVerified) return `同 Run 已验证${evidence.runId ? ` · ${evidence.runId.slice(0, 8)}` : ""}`;
  return "本地导入上下文 · Flow 身份未验证";
}

function evidenceShortSource(evidence?: ProfileEvidence) {
  if (!evidence) return "无来源";
  return evidence.sameRunVerified ? "同 Run" : "未验证导入";
}

function EvidenceDetails({ evidence, result, runtimeCollector, rawFile }: { evidence?: ProfileEvidence; result?: NormalizedResult; runtimeCollector?: string; rawFile?: string }) {
  const collector = evidence?.collector ?? runtimeCollector ?? result?.adapter ?? "未记录";
  const sourceFile = evidence?.rawFile ?? rawFile ?? result?.source.rawFile;
  const flowHash = evidence?.flowHash ?? result?.flowHash;
  return (
    <details className="evidence-details">
      <summary>查看证据详情</summary>
      <dl>
        <div><dt>来源状态</dt><dd>{evidence ? evidenceStateLabel(evidence) : result ? "受管 Run 证据" : "未记录"}</dd></div>
        <div><dt>Job ID</dt><dd>{evidence?.jobId ?? result?.jobId ?? "本地文件无 Job ID"}</dd></div>
        <div><dt>Run ID</dt><dd>{evidence?.runId ?? result?.runId ?? "本地文件无 Run ID"}</dd></div>
        <div><dt>Profile ID</dt><dd>{evidence?.report?.profileId ?? "不适用"}</dd></div>
        <div><dt>Flow Hash</dt><dd>{flowHash ?? "未验证"}</dd></div>
        <div><dt>Collector</dt><dd>{collector}</dd></div>
        <div><dt>Producer / 版本</dt><dd>{evidence ? `${evidence.producer ?? "未记录"} / ${evidence.producerVersion ?? "未记录"}` : result?.source.name ?? "未记录"}</dd></div>
        <div><dt>原始文件</dt><dd>{sourceFile ?? evidence?.fileName ?? "未记录"}</dd></div>
        <div><dt>定义版本</dt><dd>{result?.androidNative?.definitionsVersion ?? result?.iosNative?.definitionsVersion ?? evidence?.producerVersion ?? "未记录"}</dd></div>
        <div><dt>成功迭代</dt><dd>{result ? `${result.summary.successfulIterationCount} / ${result.summary.iterationCount}` : "本地文件不含迭代"}</dd></div>
      </dl>
    </details>
  );
}

function profileTypeLabel(report: DiagnosticProfileReport) {
  return report.profileType === "react_profiler" ? "React Profiler" : "Hermes CPU Profile";
}

function sourceLabel(source?: SourceLocation) {
  if (!source) return "";
  return `${source.file}${source.line === undefined ? "" : `:${source.line}${source.column === undefined ? "" : `:${source.column}`}`}`;
}

function formatMs(value: number) {
  return `${value.toFixed(value >= 100 ? 0 : 1)} ms`;
}

function signed(value?: number, suffix = "") {
  if (value === undefined) return "—";
  return `${value > 0 ? "+" : ""}${value.toFixed(1)}${suffix}`;
}

function changeEvidenceLabel(change: ProfileCommit["changes"][number]) {
  if (change.isFirstMount) return "首次挂载";
  const parts = [
    change.props.length ? `Props: ${change.props.join(", ")}` : "",
    change.state.length ? `State: ${change.state.join(", ")}` : "",
    change.context.length ? `Context: ${change.context.join(", ")}` : "",
    change.didHooksChange || change.hooks.length ? `Hooks${change.hooks.length ? `: ${change.hooks.join(", ")}` : ""}` : "",
  ].filter(Boolean);
  return parts.join(" · ") || "有变化记录，但字段摘要为空";
}
