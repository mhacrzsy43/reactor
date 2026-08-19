import {
  AlertTriangle,
  Braces,
  Check,
  Flame,
  GitCompare,
  Layers3,
  RefreshCw,
  Timer,
  Upload,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import type { ChangeEvent } from "react";
import { analyzeProfileJson, diffProfileReports } from "./api";
import type {
  ComponentProfileStat,
  DiagnosticProfileReport,
  ProfileCommit,
  ProfileDiffReport,
  SourceLocation,
} from "./types";

type DiagnosticView = "components" | "timeline" | "flame" | "findings" | "functions" | "diff";

export function DiagnosticCenter() {
  const [currentName, setCurrentName] = useState("");
  const [baselineName, setBaselineName] = useState("");
  const [sourceMapName, setSourceMapName] = useState("");
  const [sourceMapJson, setSourceMapJson] = useState("");
  const [currentJson, setCurrentJson] = useState("");
  const [baselineJson, setBaselineJson] = useState("");
  const [current, setCurrent] = useState<DiagnosticProfileReport>();
  const [baseline, setBaseline] = useState<DiagnosticProfileReport>();
  const [diff, setDiff] = useState<ProfileDiffReport>();
  const [view, setView] = useState<DiagnosticView>("components");
  const [selectedCommit, setSelectedCommit] = useState<ProfileCommit>();
  const [loading, setLoading] = useState<"current" | "baseline" | "source-map">();
  const [error, setError] = useState("");

  useEffect(() => {
    if (!baseline || !current) {
      setDiff(undefined);
      return;
    }
    void diffProfileReports(baseline, current)
      .then(setDiff)
      .catch((reason) => setError(`Profile 对比失败：${String(reason)}`));
  }, [baseline, current]);

  async function loadProfile(kind: "current" | "baseline", file?: File) {
    if (!file) return;
    setLoading(kind);
    setError("");
    try {
      const json = await file.text();
      const report = await analyzeProfileJson(json, sourceMapJson || undefined);
      if (kind === "current") {
        setCurrentJson(json);
        setCurrent(report);
        setCurrentName(file.name);
        setSelectedCommit(undefined);
        setView(report.profileType === "hermes_cpu" ? "functions" : "components");
      } else {
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
      const [nextCurrent, nextBaseline] = await Promise.all([
        currentJson ? analyzeProfileJson(currentJson, mapJson) : Promise.resolve(undefined),
        baselineJson ? analyzeProfileJson(baselineJson, mapJson) : Promise.resolve(undefined),
      ]);
      setSourceMapName(file.name);
      setSourceMapJson(mapJson);
      if (nextCurrent) setCurrent(nextCurrent);
      if (nextBaseline) setBaseline(nextBaseline);
    } catch (reason) {
      setError(`无法应用 ${file.name}：${String(reason)}`);
    } finally {
      setLoading(undefined);
    }
  }

  function onFile(kind: "current" | "baseline", event: ChangeEvent<HTMLInputElement>) {
    void loadProfile(kind, event.target.files?.[0]);
    event.target.value = "";
  }

  function onSourceMap(event: ChangeEvent<HTMLInputElement>) {
    void loadSourceMap(event.target.files?.[0]);
    event.target.value = "";
  }

  const tabs = useMemo(() => {
    if (!current) return [];
    if (current.profileType === "hermes_cpu") {
      return [
        ["functions", "JS 热点", Flame],
        ["findings", "规则发现", AlertTriangle],
      ] as const;
    }
    return [
      ["components", "组件榜单", Layers3],
      ["timeline", "Commit 时间线", Timer],
      ["flame", "火焰视图", Flame],
      ["findings", "重复渲染", AlertTriangle],
      ["diff", "Profile Diff", GitCompare],
    ] as const;
  }, [current]);

  return (
    <>
      <header className="topbar diagnostic-topbar">
        <div>
          <p className="eyebrow">COMPONENT DIAGNOSTICS</p>
          <h1>组件渲染诊断</h1>
        </div>
        <div className="top-actions">
          <span className="status-pill ready"><span className="status-dot" />本地解析 · 不上传源码</span>
        </div>
      </header>

      {error && <div className="error-banner">{error}</div>}

      <section className="diagnostic-import card">
        <div className="card-heading">
          <div className="heading-icon purple"><Braces size={19} /></div>
          <div>
            <h2>导入诊断证据</h2>
            <p>支持 React DevTools Profiler 与 Hermes / Chrome CPU Profile JSON</p>
          </div>
          <span className="schema-badge">PROFILE v1</span>
        </div>
        <div className="diagnostic-files">
          <ProfileFilePicker
            label="当前 Profile"
            name={currentName}
            report={current}
            loading={loading === "current"}
            required
            onChange={(event) => onFile("current", event)}
          />
          <GitCompare size={18} className="diagnostic-file-arrow" />
          <ProfileFilePicker
            label="基线 Profile（可选）"
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
            {sourceMapJson && <small>{current?.sourceMapMappedCount ?? 0} 个位置已映射</small>}
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
      </section>

      {!current ? (
        <section className="diagnostic-empty card">
          <Upload size={26} />
          <h2>导入一个 Profile 开始诊断</h2>
          <p>Reactor 会统计每个组件的 Render/Commit 次数、Self Time，并用规则检查重复渲染。</p>
        </section>
      ) : (
        <>
          <ProfileSummary report={current} />
          <section className="diagnostic-workbench card">
            <div className="diagnostic-tabs" role="tablist" aria-label="诊断视图">
              {tabs.map(([id, label, Icon]) => (
                <button
                  key={id}
                  className={view === id ? "active" : ""}
                  disabled={id === "diff" && !diff}
                  onClick={() => setView(id)}
                  role="tab"
                  aria-selected={view === id}
                >
                  <Icon size={14} />{label}
                  {id === "findings" && <b>{current.findings.length}</b>}
                  {id === "diff" && diff && <b>{diff.regressionCount}</b>}
                </button>
              ))}
            </div>

            {view === "components" && <ComponentTable components={current.components} selectedCommit={selectedCommit} />}
            {view === "timeline" && (
              <CommitTimeline
                commits={current.commits}
                selected={selectedCommit}
                components={current.components}
                onSelect={setSelectedCommit}
              />
            )}
            {view === "flame" && <ComponentFlame components={current.components} />}
            {view === "findings" && <Findings report={current} />}
            {view === "functions" && <FunctionHotspots report={current} />}
            {view === "diff" && diff && <ProfileDiff diff={diff} baselineName={baselineName} currentName={currentName} />}
          </section>
        </>
      )}
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
        <b>{name || (required ? "尚未选择" : "用于检查次数和耗时回归")}</b>
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

function ProfileSummary({ report }: { report: DiagnosticProfileReport }) {
  const renderCount = report.components.reduce((sum, component) => sum + component.renderCount, 0);
  const selfTime = report.components.reduce((sum, component) => sum + component.selfTimeMs, 0);
  const cards = report.profileType === "react_profiler"
    ? [
        ["组件", report.components.length, "个"],
        ["Render", renderCount, "次"],
        ["Commit", report.commitCount, "次"],
        ["Self Time", selfTime.toFixed(1), "ms"],
        ["规则发现", report.findings.length, "项"],
      ]
    : [
        ["JS 函数", report.functions.length, "个"],
        ["采样", report.functions.reduce((sum, fn) => sum + fn.sampleCount, 0), "次"],
        ["总时长", report.totalDurationMs.toFixed(1), "ms"],
        ["热点", report.findings.length, "项"],
      ];
  return (
    <section className="diagnostic-summary">
      {cards.map(([label, value, unit]) => (
        <div className="card" key={label}>
          <span>{label}</span><strong>{value}</strong><small>{unit}</small>
        </div>
      ))}
    </section>
  );
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
}: {
  commits: ProfileCommit[];
  selected?: ProfileCommit;
  components: ComponentProfileStat[];
  onSelect: (commit: ProfileCommit) => void;
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

function Findings({ report }: { report: DiagnosticProfileReport }) {
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
