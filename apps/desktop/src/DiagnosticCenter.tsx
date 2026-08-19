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
import { Component, useEffect, useState } from "react";
import type { ChangeEvent, ReactNode } from "react";
import { analyzeManagedProfile, analyzeProfileJson, diffProfileReports, getJobSnapshot, listJobs } from "./api";
import type {
  ComponentProfileStat,
  DiagnosticProfileReport,
  NormalizedResult,
  ProfileCommit,
  ProfileDiffReport,
  SourceLocation,
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

function DiagnosticCenterContent({ activeFlow, onNavigate }: DiagnosticCenterProps) {
  const [framework, setFramework] = useState<DiagnosticFramework>(activeFlow?.framework ?? "react-native");
  const [currentName, setCurrentName] = useState("");
  const [hermesName, setHermesName] = useState("");
  const [baselineName, setBaselineName] = useState("");
  const [sourceMapName, setSourceMapName] = useState("");
  const [sourceMapJson, setSourceMapJson] = useState("");
  const [currentJson, setCurrentJson] = useState("");
  const [hermesJson, setHermesJson] = useState("");
  const [baselineJson, setBaselineJson] = useState("");
  const [current, setCurrent] = useState<DiagnosticProfileReport>();
  const [hermes, setHermes] = useState<DiagnosticProfileReport>();
  const [baseline, setBaseline] = useState<DiagnosticProfileReport>();
  const [diff, setDiff] = useState<ProfileDiffReport>();
  const [view, setView] = useState<DiagnosticView>("overview");
  const [selectedCommit, setSelectedCommit] = useState<ProfileCommit>();
  const [loading, setLoading] = useState<"current" | "hermes" | "baseline" | "source-map" | "runs" | undefined>("runs");
  const [latestRuns, setLatestRuns] = useState<Partial<Record<DiagnosticFramework, NormalizedResult>>>({});
  const [recentRuns, setRecentRuns] = useState<NormalizedResult[]>([]);
  const [profileFlowHash, setProfileFlowHash] = useState("");
  const [error, setError] = useState("");

  useEffect(() => {
    if (activeFlow) setFramework(activeFlow.framework);
  }, [activeFlow?.flowHash, activeFlow?.framework]);

  useEffect(() => {
    if (!baseline || !current) {
      setDiff(undefined);
      return;
    }
    void diffProfileReports(baseline, current)
      .then(setDiff)
      .catch((reason) => setError(`Profile 对比失败：${String(reason)}`));
  }, [baseline, current]);

  useEffect(() => {
    let cancelled = false;
    void listJobs(30, 0).then(async (page) => {
      const jobs = page.jobs.filter((job) => job.state === "completed" && job.resultPath).slice(0, 20);
      const snapshots = await Promise.all(jobs.map((job) => getJobSnapshot(job.id, { limit: 1 }).catch(() => undefined)));
      const next: Partial<Record<DiagnosticFramework, NormalizedResult>> = {};
      for (const snapshot of snapshots) {
        for (const result of snapshot?.results ?? []) {
          const key = normalizeFramework(result.framework);
          if (key && !next[key] && !result.source.synthetic) next[key] = result;
        }
      }
      if (!cancelled) {
        setLatestRuns(next);
        setRecentRuns(snapshots.flatMap((snapshot) => snapshot?.results ?? []).filter((result) => !result.source.synthetic));
      }
    }).catch((reason) => {
      if (!cancelled) setError(`读取最近性能结果失败：${String(reason)}`);
    }).finally(() => {
      if (!cancelled) setLoading((value) => value === "runs" ? undefined : value);
    });
    return () => { cancelled = true; };
  }, []);

  async function loadProfile(kind: "current" | "hermes" | "baseline", file?: File) {
    if (!file) return;
    setLoading(kind);
    setError("");
    try {
      const json = await file.text();
      const report = await analyzeProfileJson(json, sourceMapJson || undefined);
      if (kind === "current") {
        if (report.profileType !== "react_profiler") throw new Error("这里需要 React DevTools Profiler JSON；Hermes CPU Profile 请导入到独立的 JS 热点栏");
        setCurrentJson(json);
        setCurrent(report);
        setCurrentName(file.name);
        setProfileFlowHash(activeFlow?.flowHash ?? selectedResult?.flowHash ?? "");
        setSelectedCommit(undefined);
        setView("overview");
      } else if (kind === "hermes") {
        if (report.profileType !== "hermes_cpu") throw new Error("这里需要 Hermes / Chrome CPU Profile；React Profile 请导入到 Render 栏");
        setHermesJson(json);
        setHermes(report);
        setHermesName(file.name);
        setProfileFlowHash(activeFlow?.flowHash ?? selectedResult?.flowHash ?? "");
        setView("hermes");
      } else {
        if (report.profileType !== "react_profiler") throw new Error("Profile Diff 基线需要 React DevTools Profiler JSON");
        setBaselineJson(json);
        setBaseline(report);
        setBaselineName(file.name);
      }
    } catch (reason) {
      setError(`无法导入 ${file.name}：${String(reason)}`);
    } finally {
      setLoading(undefined);
    }
  }

  async function loadSourceMap(file?: File) {
    if (!file) return;
    setLoading("source-map");
    setError("");
    try {
      const mapJson = await file.text();
      const [nextCurrent, nextHermes, nextBaseline] = await Promise.all([
        currentJson ? analyzeProfileJson(currentJson, mapJson) : Promise.resolve(undefined),
        hermesJson ? analyzeProfileJson(hermesJson, mapJson) : Promise.resolve(undefined),
        baselineJson ? analyzeProfileJson(baselineJson, mapJson) : Promise.resolve(undefined),
      ]);
      setSourceMapName(file.name);
      setSourceMapJson(mapJson);
      if (nextCurrent) setCurrent(nextCurrent);
      if (nextHermes) setHermes(nextHermes);
      if (nextBaseline) setBaseline(nextBaseline);
    } catch (reason) {
      setError(`无法应用 ${file.name}：${String(reason)}`);
    } finally {
      setLoading(undefined);
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
    ["runtime", "实时组件 / 日志", Braces],
    ["render", "Render", Layers3],
    ["findings", "重复渲染", AlertTriangle],
    ["hermes", "Hermes / JS CPU", Cpu],
    ["timeline", "时间线 / 火焰图", Timer],
    ["diff", "Profile Diff", GitCompare],
    ["source", "源码定位", MapPinned],
  ] as const;

  function drill(target: DiagnosticView, commitId?: string) {
    if (commitId && current) setSelectedCommit(current.commits.find((commit) => commit.id === commitId));
    setView(target);
  }

  const selectedResult = activeFlow
    ? recentRuns.find((result) => result.flowHash === activeFlow.flowHash && normalizeFramework(result.framework) === framework)
    : latestRuns[framework];

  useEffect(() => {
    const profileFile = selectedResult?.androidNative?.rnDiagnostics?.profileFile;
    if (!profileFile) return;
    let cancelled = false;
    setLoading("current");
    void analyzeManagedProfile(profileFile)
      .then((report) => {
        if (cancelled) return;
        setCurrent(report);
        setCurrentName("Flow 自动采集 · RN Profiling Renderer");
        setProfileFlowHash(selectedResult.flowHash);
        setSelectedCommit(undefined);
      })
      .catch((reason) => {
        if (!cancelled) setError(`自动解析 Flow RN Profile 失败：${String(reason)}`);
      })
      .finally(() => {
        if (!cancelled) setLoading((value) => value === "current" ? undefined : value);
      });
    return () => { cancelled = true; };
  }, [selectedResult?.flowHash, selectedResult?.androidNative?.rnDiagnostics?.profileFile]);

  return (
    <>
      <header className="topbar diagnostic-topbar">
        <div>
          <p className="eyebrow">PERFORMANCE DIAGNOSTICS</p>
          <h1>性能诊断</h1>
        </div>
        <div className="top-actions">
          <span className="status-pill ready"><span className="status-dot" />本地解析 · 不上传源码</span>
        </div>
      </header>

      {error && <div className="error-banner">{error}</div>}

      <section className={`diagnostic-flow-context card ${activeFlow ? selectedResult ? "linked" : "pending" : "unbound"}`}>
        <div>
          <span>{activeFlow ? "当前锁定 Flow" : "尚未锁定当前 Flow"}</span>
          <b>{activeFlow?.name ?? "性能诊断暂时展示各框架最近一次可信运行"}</b>
          <small>{activeFlow ? `${activeFlow.appId} · ${activeFlow.flowHash.slice(0, 12)}…` : "锁定 Flow 后，Reactor 会按 Flow Hash 自动关联采集结果。"}</small>
        </div>
        <div>
          <strong>{activeFlow ? selectedResult ? "已关联同 Flow Run" : "尚未按此 Flow 采集" : "最近可信 Run"}</strong>
          {selectedResult && <code>{selectedResult.runId.slice(0, 12)}… · {selectedResult.platform}</code>}
          {onNavigate && <button className="secondary-button" onClick={() => onNavigate(activeFlow && selectedResult ? "history" : "flow")}>{activeFlow && selectedResult ? "查看原始运行" : activeFlow ? "返回 Flow 开始采集" : "前往 Flow Studio"}</button>}
        </div>
      </section>

      <section className="diagnostic-frameworks card" aria-label="框架选择">
        <div><b>选择框架</b><span>黑盒性能统一呈现，框架专项证据按能力接入</span></div>
        <div role="tablist">
          {(["react-native", "flutter", "lynx"] as DiagnosticFramework[]).map((item) => (
            <button key={item} className={framework === item ? "active" : ""} onClick={() => { setFramework(item); setView("overview"); }}>
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
        {view === "overview" && <PerformanceOverview framework={framework} result={selectedResult} current={current} hermes={hermes} loading={loading === "runs"} onDrill={drill} onNavigate={onNavigate} />}
        {framework !== "react-native" && view !== "overview" && <FrameworkPending framework={framework} onOverview={() => setView("overview")} />}
        {framework === "react-native" && view === "runtime" && <RuntimeDiagnostics result={selectedResult} />}
        {framework === "react-native" && view === "render" && (current ? <ComponentTable components={current.components} selectedCommit={selectedCommit} /> : <EvidenceEmpty title="尚未采集 React Render Profile" detail="从 React Native DevTools 的 Profiler 导出 JSON，导入后可查看每个组件的 Render/Commit 次数与耗时。" />)}
        {framework === "react-native" && view === "findings" && (current ? <Findings report={current} onDrill={(commitId) => drill(commitId ? "timeline" : "render", commitId)} /> : <EvidenceEmpty title="重复渲染需要 React Profile 证据" detail="导入后 Reactor 会识别无变化 Render、父组件级联，并给出组件和 Commit 引用。" />)}
        {framework === "react-native" && view === "hermes" && (hermes ? <FunctionHotspots report={hermes} /> : <EvidenceEmpty title="尚未采集 Hermes / JS CPU Profile" detail="导入 Hermes 或 Chrome CPU Profile，查看 JS Self Time、采样数和 Source Map 源码位置。" />)}
        {framework === "react-native" && view === "timeline" && (current ? <><CommitTimeline commits={current.commits} selected={selectedCommit} components={current.components} onSelect={setSelectedCommit} onInspectComponents={() => setView("render")} /><ComponentFlame components={current.components} /></> : <EvidenceEmpty title="时间线与火焰图等待 Profile" detail="导入 React Profile 后，可从异常 Commit 下钻到参与渲染的组件与父子耗时层级。" />)}
        {framework === "react-native" && view === "diff" && (diff ? <ProfileDiff diff={diff} baselineName={baselineName} currentName={currentName} /> : <EvidenceEmpty title="Profile Diff 需要当前与基线" detail="同时导入两份 React Profile，Reactor 会比较组件 Render 次数和累计耗时回归。" />)}
        {framework === "react-native" && view === "source" && <SourceMapPanel sourceMapName={sourceMapName} sourceMapJson={sourceMapJson} mappedCount={(current?.sourceMapMappedCount ?? 0) + (hermes?.sourceMapMappedCount ?? 0)} loading={loading === "source-map"} onChange={onSourceMap} />}
      </section>

      {framework === "react-native" && <section id="rn-profile-import" className="diagnostic-import card">
        <div className="card-heading">
          <div className="heading-icon purple"><Braces size={19} /></div>
          <div>
            <h2>采集或导入 RN 诊断证据</h2>
            <p>{activeFlow ? `导入后绑定当前 Flow ${activeFlow.flowHash.slice(0, 12)}…；黑盒 Run 已按同一 Hash 自动关联` : "请先锁定 Flow；React DevTools/Hermes Profile 导入后会绑定该 Flow"}</p>
          </div>
          <span className="schema-badge">{profileFlowHash ? `FLOW ${profileFlowHash.slice(0, 8)}` : "PROFILE v1"}</span>
        </div>
        <div className="diagnostic-files">
          <ProfileFilePicker
            label="React Render Profile"
            name={currentName}
            report={current}
            loading={loading === "current"}
            required
            onChange={(event) => onFile("current", event)}
          />
          <ProfileFilePicker
            label="Hermes / JS CPU Profile"
            name={hermesName}
            report={hermes}
            loading={loading === "hermes"}
            onChange={(event) => onFile("hermes", event)}
          />
          <ProfileFilePicker
            label="React 基线（可选）"
            name={baselineName}
            report={baseline}
            loading={loading === "baseline"}
            onChange={(event) => onFile("baseline", event)}
          />
        </div>
        <div className={`diagnostic-source-map ${sourceMapJson ? "loaded" : ""}`}>
          <div>
            <span>Source Map（可选）</span>
            <b>{sourceMapName || "将 bundle.js 位置映射回 TypeScript / TSX 源码"}</b>
            {sourceMapJson && <small>{(current?.sourceMapMappedCount ?? 0) + (hermes?.sourceMapMappedCount ?? 0)} 个位置已映射</small>}
          </div>
          <label className="secondary-button diagnostic-upload">
            {loading === "source-map" ? <RefreshCw size={14} className="spin" /> : sourceMapJson ? <Check size={14} /> : <Upload size={14} />}
            {sourceMapJson ? "更换 Map" : "选择 .map"}
            <input type="file" accept=".json,.map" onChange={onSourceMap} disabled={loading === "source-map"} />
          </label>
        </div>
        <p className="diagnostic-privacy">
          Profile、Source Location 和调用栈只在本机 Rust 核心中解析；本页不会调用 AI。
        </p>
      </section>}
    </>
  );
}

function ProfileFilePicker({
  label,
  name,
  report,
  loading,
  required,
  onChange,
}: {
  label: string;
  name: string;
  report?: DiagnosticProfileReport;
  loading: boolean;
  required?: boolean;
  onChange: (event: ChangeEvent<HTMLInputElement>) => void;
}) {
  return (
    <div className={`diagnostic-file ${report ? "loaded" : ""}`}>
      <div>
        <span>{label}</span>
        <b>{name || (required ? "尚未选择" : label.includes("Hermes") ? "尚未导入 JS CPU 证据" : "用于检查次数和耗时回归")}</b>
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
  current,
  hermes,
  loading,
  onDrill,
  onNavigate,
}: {
  framework: DiagnosticFramework;
  result?: NormalizedResult;
  current?: DiagnosticProfileReport;
  hermes?: DiagnosticProfileReport;
  loading: boolean;
  onDrill: (target: DiagnosticView) => void;
  onNavigate?: (page: "flow" | "history" | "analysis") => void;
}) {
  const frameP95 = firstFiniteMetric(result?.androidNative?.frameTimeP95Ms, result?.iosNative?.frameTimeP95Ms);
  const cpu = firstFiniteMetric(result?.summary.cpuMeanPct, result?.iosNative?.cpuMeanPct);
  const memory = firstFiniteMetric(result?.androidNative?.memoryPssMb, result?.iosNative?.memoryPeakMb, result?.summary.ramPeakMb);
  const startup = firstFiniteMetric(result?.androidNative?.startupTimeMs, result?.iosNative?.startupTimeMs);
  const metrics: Array<[string, number | undefined, string, DiagnosticView, string]> = [
    ["P95 帧耗时", frameP95, "ms", "timeline", "下钻 Commit 时间线"],
    ["Jank", firstFiniteMetric(result?.androidNative?.jankFramePct), "%", "timeline", "下钻渲染时间线"],
    ["CPU", cpu, "%", "hermes", "下钻 JS CPU 热点"],
    ["内存峰值", memory, "MB", "render", "检查组件渲染路径"],
    ["冷启动", startup, "ms", "render", "检查首次挂载组件"],
  ];
  return (
    <div className="diagnostic-panel performance-overview">
      <div className="diagnostic-panel-heading">
        <div><h2>{frameworkLabel(framework)} 性能总览</h2><p>最近一次可信运行的黑盒指标，与框架专项 Profile 证据在同一处下钻。</p></div>
        {result && <span>{result.platform} · {result.device?.physical ? "物理设备" : "模拟器"}</span>}
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
        <div className="diagnostic-run-empty"><Activity size={21} /><div><b>还没有 {frameworkLabel(framework)} 的可信运行结果</b><span>先完成一次快速验收或正式基准，性能总览会自动读取最新证据。</span></div>{onNavigate && <button className="secondary-button" onClick={() => onNavigate("history")}>查看运行记录</button>}</div>
      )}
      {framework === "react-native" ? (
        <div className="rn-evidence-status">
          <button className={current ? "ready" : ""} onClick={() => onDrill("render")}><Layers3 size={17} /><div><b>React Render</b><span>{current ? `${current.components.length} 个组件 · ${current.commitCount} commits` : "待导入 React Profiler"}</span></div><ChevronRight size={15} /></button>
          <button className={current?.findings.length ? "warning" : current ? "ready" : ""} onClick={() => onDrill("findings")}><AlertTriangle size={17} /><div><b>重复渲染</b><span>{current ? `${current.findings.length} 项规则发现` : "待采集组件证据"}</span></div><ChevronRight size={15} /></button>
          <button className={hermes ? "ready" : ""} onClick={() => onDrill("hermes")}><Cpu size={17} /><div><b>Hermes / JS CPU</b><span>{hermes ? `${hermes.functions.length} 个函数热点` : "待导入 CPU Profile"}</span></div><ChevronRight size={15} /></button>
          <button className={current?.sourceMapApplied || hermes?.sourceMapApplied ? "ready" : ""} onClick={() => onDrill("source")}><MapPinned size={17} /><div><b>源码定位</b><span>{current?.sourceMapApplied || hermes?.sourceMapApplied ? "Source Map 已应用" : "待导入 Source Map"}</span></div><ChevronRight size={15} /></button>
        </div>
      ) : <FrameworkRoadmap framework={framework} />}
      {result && onNavigate && <div className="diagnostic-overview-actions"><button className="secondary-button" onClick={() => onNavigate("history")}>查看原始运行</button><button className="secondary-button" onClick={() => onNavigate("analysis")}>进行回归对比</button></div>}
    </div>
  );
}

function RuntimeDiagnostics({ result }: { result?: NormalizedResult }) {
  const evidence = result?.androidNative?.rnDiagnostics;
  if (!evidence) return <EvidenceEmpty title="当前 Run 没有 RN 受管诊断证据" detail="目标 App 需要接入 Reactor RN SDK；Release Benchmark 仍可运行，但不会伪造组件树、Console、Network 或对象保留证据。" />;
  const recentEvents = evidence.recentEvents ?? [];
  const latestTreeEvent = [...recentEvents].reverse().find((event) => event.kind === "component_tree");
  const treeNodes = Array.isArray(latestTreeEvent?.payload.nodes) ? latestTreeEvent.payload.nodes as Array<Record<string, unknown>> : [];
  const timeline = recentEvents.filter((event) => ["component_render", "component_tree", "react_profile", "console", "network", "object_lifecycle", "hermes_heap"].includes(event.kind)).slice(-24);
  return (
    <div className="diagnostic-panel runtime-diagnostics">
      <div className="diagnostic-panel-heading"><div><h2>Flow 自动绑定的 RN 运行时证据</h2><p>{evidence.collector} · {evidence.benchmarkMode ?? "未声明模式"} · {evidence.eventCount} 条本地事件</p></div><span>{evidence.profileCommitCount ? "Profiling Renderer" : "生产 Renderer · 无耗时 Profile"}</span></div>
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

function EvidenceEmpty({ title, detail }: { title: string; detail: string }) {
  return <div className="diagnostic-empty inline"><Upload size={24} /><h2>{title}</h2><p>{detail}</p><button className="secondary-button" onClick={() => document.getElementById("rn-profile-import")?.scrollIntoView({ behavior: "smooth" })}>查看采集与导入入口</button></div>;
}

function FrameworkPending({ framework, onOverview }: { framework: DiagnosticFramework; onOverview: () => void }) {
  return <div className="diagnostic-empty inline"><Layers3 size={24} /><h2>{frameworkLabel(framework)} 专项诊断正在接入</h2><p>黑盒 FPS、帧耗时、CPU、内存和启动指标已经进入统一总览；当前不会用 RN 的组件语义冒充 {frameworkLabel(framework)} 专项证据。</p><button className="secondary-button" onClick={onOverview}>返回性能总览</button></div>;
}

function FrameworkRoadmap({ framework }: { framework: DiagnosticFramework }) {
  const items = framework === "flutter" ? ["Flutter DevTools Timeline", "Widget rebuild 统计", "Dart CPU / Allocation", "Shader / Raster jank"] : ["Lynx Trace / Timing", "组件更新次数", "JS / Native 双线程热点", "源码映射"];
  return <div className="framework-roadmap"><div><b>{frameworkLabel(framework)} 专项接入边界</b><span>以下能力登记在同一工作台，未采集前不生成占位结论。</span></div>{items.map((item) => <span key={item}>{item}<small>待接入</small></span>)}</div>;
}

function SourceMapPanel({ sourceMapName, sourceMapJson, mappedCount, loading, onChange }: { sourceMapName: string; sourceMapJson: string; mappedCount: number; loading: boolean; onChange: (event: ChangeEvent<HTMLInputElement>) => void }) {
  return <div className="diagnostic-panel"><div className="diagnostic-panel-heading"><div><h2>Source Map / 源码定位</h2><p>把 Hermes bundle 行列和组件位置映射回 TypeScript / TSX。</p></div><span>{mappedCount} 个位置已映射</span></div><div className={`diagnostic-source-map source-panel ${sourceMapJson ? "loaded" : ""}`}><MapPinned size={20} /><div><span>当前 Source Map</span><b>{sourceMapName || "尚未导入 .map 文件"}</b><small>{sourceMapJson ? "已在本机重新解析所有已导入 Profile" : "不会上传源码或调用 AI"}</small></div><label className="secondary-button diagnostic-upload">{loading ? <RefreshCw size={14} className="spin" /> : sourceMapJson ? <Check size={14} /> : <Upload size={14} />}{sourceMapJson ? "更换 Map" : "选择 .map"}<input type="file" accept=".json,.map" onChange={onChange} disabled={loading} /></label></div></div>;
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
              <small>{component.unchangedRenderCount} 次无变化 · {component.updaterCount} 次触发更新</small>
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
      <div className="diagnostic-panel-heading"><div><h2>确定性规则发现</h2><p>不调用 AI；每项发现都引用组件、Commit 和原始 Profile 字段。</p></div></div>
      {report.findings.length === 0 ? (
        <div className="diagnostic-ok"><Check size={18} /><div><b>没有命中当前规则</b><span>这不代表不存在其他性能问题，可继续查看时间线和耗时榜单。</span></div></div>
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
  baselineName,
  currentName,
}: {
  diff: ProfileDiffReport;
  baselineName: string;
  currentName: string;
}) {
  return (
    <div className="diagnostic-panel">
      <div className="diagnostic-panel-heading">
        <div><h2>组件 Profile Diff</h2><p>{baselineName} → {currentName}</p></div>
        <span className={diff.regressionCount ? "diff-regressed" : "diff-stable"}>{diff.regressionCount} 项回归</span>
      </div>
      {!diff.compatible && <div className="error-banner inline">{diff.reasons.join("；")}</div>}
      <div className="profile-diff-list">
        {diff.components.map((component) => (
          <div className={component.regressed ? "regressed" : ""} key={component.key}>
            <div><b>{component.name}</b><small>{sourceLabel(component.source) || component.key}</small></div>
            <span>Render <b>{component.baselineRenderCount} → {component.currentRenderCount}</b><small>{signed(component.renderCountDelta)} 次</small></span>
            <span>Total <b>{formatMs(component.baselineTotalTimeMs)} → {formatMs(component.currentTotalTimeMs)}</b><small>{signed(component.totalTimeDeltaPct, "%")}</small></span>
            <strong>{component.newComponent ? "新增" : component.removedComponent ? "消失" : component.regressed ? "回归" : "稳定"}</strong>
          </div>
        ))}
      </div>
    </div>
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
