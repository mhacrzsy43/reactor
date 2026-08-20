import { AlertTriangle, Check, ChevronLeft, ChevronRight, Crosshair, Hand, List, LoaderCircle, Minus, Plus, RotateCcw, Search } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent, WheelEvent as ReactWheelEvent } from "react";
import { analyzeDiagnosticSelection, getDiagnosticManifest, getFrameDrilldown, getTimelineOverview, getTimelineWindow } from "./api";
import {
  availabilityLabel,
  canvasXAtTime,
  formatTimelineTime,
  itemIntersectsRange,
  normalizeTimelineRange,
  panTimelineRange,
  TIMELINE_TRACK_LABELS,
  TIMELINE_TRACKS,
  timeAtCanvasX,
  timelineItemLabel,
  trackAvailability,
  zoomTimelineRange,
} from "./timelineLogic";
import type {
  DiagnosticCorrelationCandidate,
  DiagnosticManifest,
  DiagnosticSelectionAnalysis,
  FrameDrilldown,
  TimelineItem,
  TimelineOverview,
  TimelineRange,
  TimelineTrackKind,
  TimelineWindow,
} from "./types";

interface UnifiedTimelineProps {
  jobId?: string;
  runId?: string;
}

type InteractionMode = "pan" | "brush";
type DragState = { mode: InteractionMode; startX: number; lastX: number; startMs: number; currentMs: number };

const TRACK_HEIGHT = 48;
const RULER_HEIGHT = 27;
const CANVAS_MIN_WIDTH = 320;
const WINDOW_QUERY_DELAY_MS = 90;

export function UnifiedTimeline({ jobId, runId }: UnifiedTimelineProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const queryToken = useRef(0);
  const [manifest, setManifest] = useState<DiagnosticManifest>();
  const [viewport, setViewport] = useState<TimelineRange>();
  const [overview, setOverview] = useState<TimelineOverview>();
  const [windowData, setWindowData] = useState<TimelineWindow>();
  const [selectedItem, setSelectedItem] = useState<TimelineItem>();
  const [selection, setSelection] = useState<TimelineRange>();
  const [selectionAnalysis, setSelectionAnalysis] = useState<DiagnosticSelectionAnalysis>();
  const [frameDrilldown, setFrameDrilldown] = useState<FrameDrilldown>();
  const [mode, setMode] = useState<InteractionMode>("pan");
  const [drag, setDrag] = useState<DragState>();
  const [canvasSize, setCanvasSize] = useState({ width: 900, height: RULER_HEIGHT + TRACK_HEIGHT * TIMELINE_TRACKS.length });
  const [manifestLoading, setManifestLoading] = useState(false);
  const [windowLoading, setWindowLoading] = useState(false);
  const [detailLoading, setDetailLoading] = useState(false);
  const [error, setError] = useState("");

  const bounds = manifest?.range;
  const availableTrackIds = useMemo(() => manifest
    ? TIMELINE_TRACKS.flatMap((kind) => {
      const track = trackAvailability(manifest, kind);
      return track.state === "available" && track.trackId !== undefined ? [track.trackId] : [];
    })
    : [], [manifest]);
  const availableTracks = useMemo(() => manifest
    ? TIMELINE_TRACKS.filter((kind) => trackAvailability(manifest, kind).state === "available")
    : [], [manifest]);
  const visibleItems = useMemo(() => {
    if (!windowData || !viewport) return [];
    return windowData.items.filter((item) => availableTracks.includes(item.track) && itemIntersectsRange(item, viewport));
  }, [availableTracks, viewport, windowData]);

  useEffect(() => {
    const element = wrapRef.current;
    if (!element) return;
    const observer = new ResizeObserver(([entry]) => {
      const width = Math.max(CANVAS_MIN_WIDTH, Math.floor(entry.contentRect.width));
      setCanvasSize((current) => current.width === width ? current : { ...current, width });
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const token = ++queryToken.current;
    setManifest(undefined);
    setViewport(undefined);
    setOverview(undefined);
    setWindowData(undefined);
    setSelectedItem(undefined);
    setSelection(undefined);
    setSelectionAnalysis(undefined);
    setFrameDrilldown(undefined);
    setError("");
    if (!jobId || !runId) return;
    setManifestLoading(true);
    void getDiagnosticManifest(jobId, runId)
      .then((next) => {
        if (queryToken.current !== token) return;
        setManifest(next);
        if (next.range && next.range.endMs > next.range.startMs) setViewport(normalizeTimelineRange(next.range, next.range));
      })
      .catch((reason) => {
        if (queryToken.current === token) setError(`无法读取诊断清单：${String(reason)}`);
      })
      .finally(() => {
        if (queryToken.current === token) setManifestLoading(false);
      });
  }, [jobId, runId]);

  useEffect(() => {
    if (!jobId || !runId || !viewport || availableTrackIds.length === 0) return;
    const token = ++queryToken.current;
    const timer = window.setTimeout(() => {
      setWindowLoading(true);
      setError("");
      void Promise.all([
        getTimelineOverview(jobId, runId, viewport, canvasSize.width),
        getTimelineWindow(jobId, runId, viewport, availableTrackIds),
      ]).then(([nextOverview, nextWindow]) => {
        if (queryToken.current !== token) return;
        setOverview(nextOverview);
        setWindowData(nextWindow);
        setSelectedItem((current) => current && nextWindow.items.some((item) => item.id === current.id) ? current : undefined);
      }).catch((reason) => {
        if (queryToken.current === token) {
          setOverview(undefined);
          setWindowData(undefined);
          setError(`无法读取当前视口时间线：${String(reason)}`);
        }
      }).finally(() => {
        if (queryToken.current === token) setWindowLoading(false);
      });
    }, WINDOW_QUERY_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [jobId, runId, viewport?.startMs, viewport?.endMs, availableTrackIds.join("|"), canvasSize.width]);

  useEffect(() => {
    drawTimeline(canvasRef.current, canvasSize, viewport, manifest, overview, visibleItems, selectedItem?.id, selection, drag?.mode === "brush" ? { startMs: drag.startMs, endMs: drag.currentMs } : undefined);
  }, [canvasSize, viewport, manifest, overview, visibleItems, selectedItem?.id, selection, drag]);

  function resetViewport() {
    if (!bounds) return;
    setViewport(normalizeTimelineRange(bounds, bounds));
    setSelection(undefined);
    setSelectionAnalysis(undefined);
  }

  function zoom(factor: number, anchor?: number) {
    if (!viewport || !bounds) return;
    setViewport(zoomTimelineRange(viewport, factor, anchor ?? (viewport.startMs + viewport.endMs) / 2, bounds));
  }

  function pan(ratio: number) {
    if (!viewport || !bounds) return;
    setViewport(panTimelineRange(viewport, (viewport.endMs - viewport.startMs) * ratio, bounds));
  }

  function pointerPosition(event: ReactPointerEvent<HTMLCanvasElement>) {
    const rect = event.currentTarget.getBoundingClientRect();
    return Math.max(0, Math.min(rect.width, event.clientX - rect.left));
  }

  function onPointerDown(event: ReactPointerEvent<HTMLCanvasElement>) {
    if (!viewport) return;
    const x = pointerPosition(event);
    const interaction = event.shiftKey ? "brush" : mode;
    const at = timeAtCanvasX(x, event.currentTarget.getBoundingClientRect().width, viewport);
    event.currentTarget.setPointerCapture(event.pointerId);
    setDrag({ mode: interaction, startX: x, lastX: x, startMs: at, currentMs: at });
  }

  function onPointerMove(event: ReactPointerEvent<HTMLCanvasElement>) {
    if (!drag || !viewport || !bounds) return;
    const x = pointerPosition(event);
    if (drag.mode === "pan") {
      const width = event.currentTarget.getBoundingClientRect().width;
      const deltaMs = -(x - drag.lastX) / width * (viewport.endMs - viewport.startMs);
      setViewport(panTimelineRange(viewport, deltaMs, bounds));
    }
    setDrag((current) => current ? {
      ...current,
      lastX: x,
      currentMs: timeAtCanvasX(x, event.currentTarget.getBoundingClientRect().width, viewport),
    } : current);
  }

  function onPointerUp(event: ReactPointerEvent<HTMLCanvasElement>) {
    if (!drag || !viewport) return;
    const x = pointerPosition(event);
    const at = timeAtCanvasX(x, event.currentTarget.getBoundingClientRect().width, viewport);
    if (drag.mode === "brush" && Math.abs(x - drag.startX) >= 3) {
      const next = normalizeTimelineRange({ startMs: drag.startMs, endMs: at });
      setSelection(next);
      setSelectedItem(undefined);
      setFrameDrilldown(undefined);
      void analyzeSelection(next);
    } else if (drag.mode === "pan" && Math.abs(x - drag.startX) < 4) {
      selectAt(event, x);
    }
    setDrag(undefined);
  }

  function onWheel(event: ReactWheelEvent<HTMLCanvasElement>) {
    if (!viewport || !bounds) return;
    event.preventDefault();
    const x = pointerPosition(event as unknown as ReactPointerEvent<HTMLCanvasElement>);
    const anchor = timeAtCanvasX(x, event.currentTarget.getBoundingClientRect().width, viewport);
    zoom(event.deltaY > 0 ? 1.25 : 0.8, anchor);
  }

  function selectAt(event: ReactPointerEvent<HTMLCanvasElement>, x: number) {
    if (!viewport) return;
    const rect = event.currentTarget.getBoundingClientRect();
    const trackIndex = Math.floor((event.clientY - rect.top - RULER_HEIGHT) / TRACK_HEIGHT);
    const kind = TIMELINE_TRACKS[trackIndex];
    if (!kind) return;
    const at = timeAtCanvasX(x, rect.width, viewport);
    const tolerance = (viewport.endMs - viewport.startMs) * 5 / rect.width;
    const item = visibleItems
      .filter((candidate) => candidate.track === kind && candidate.startMs - tolerance <= at && candidate.endMs + tolerance >= at)
      .sort((a, b) => Math.abs(a.startMs - at) - Math.abs(b.startMs - at))[0];
    if (item) void selectItem(item);
  }

  async function selectItem(item: TimelineItem) {
    setSelectedItem(item);
    setSelection({ startMs: item.startMs, endMs: item.endMs });
    setSelectionAnalysis(undefined);
    setFrameDrilldown(undefined);
    if (!jobId || !runId || item.track !== "frames") return;
    setDetailLoading(true);
    try {
      setFrameDrilldown(await getFrameDrilldown(jobId, runId, item.id));
    } catch (reason) {
      setError(`无法读取帧详情：${String(reason)}`);
    } finally {
      setDetailLoading(false);
    }
  }

  async function analyzeSelection(next: TimelineRange) {
    if (!jobId || !runId) return;
    setDetailLoading(true);
    setSelectionAnalysis(undefined);
    try {
      setSelectionAnalysis(await analyzeDiagnosticSelection(jobId, runId, next));
    } catch (reason) {
      setError(`无法分析所选时间段：${String(reason)}`);
    } finally {
      setDetailLoading(false);
    }
  }

  if (!jobId || !runId) return <TimelineState title="没有可用 Run" detail="统一时间线只查询已完成且具有可用结果的受管 Run；本地 React Profile 不包含可验证的跨轨时钟。" />;
  if (manifestLoading) return <TimelineState loading title="正在读取诊断清单" detail="加载轨道可用性和完整时间范围。" />;
  if (!manifest) return <TimelineState warning title="统一时间线不可用" detail={error || "此 Run 没有可读取的诊断清单。"} />;
  if (!bounds || bounds.endMs <= bounds.startMs) return <TimelineState warning title="时间范围不可用" detail="诊断清单没有声明有效的开始与结束时间，不能构造时间轴。" />;

  return (
    <div className="unified-timeline diagnostic-panel">
      <div className="diagnostic-panel-heading timeline-heading">
        <div><h2>统一诊断时间线</h2><p>仅请求当前视口的数据。跨轨结果只表示时间相关候选，不构成因果证明。</p></div>
        <span>{formatTimelineTime(bounds.endMs - bounds.startMs)} · {availableTracks.length}/{TIMELINE_TRACKS.length} 轨可用</span>
      </div>

      <div className="timeline-toolbar" role="toolbar" aria-label="时间线工具栏">
        <div className="timeline-tool-group" aria-label="交互模式">
          <button className={mode === "pan" ? "active" : ""} aria-pressed={mode === "pan"} onClick={() => setMode("pan")} title="拖动平移"><Hand size={14} />平移</button>
          <button className={mode === "brush" ? "active" : ""} aria-pressed={mode === "brush"} onClick={() => setMode("brush")} title="拖动选择时间段；平移模式下按 Shift 也可框选"><Crosshair size={14} />框选</button>
        </div>
        <div className="timeline-tool-group" aria-label="视口控制">
          <button onClick={() => zoom(0.7)} aria-label="放大时间线"><Plus size={14} /></button>
          <button onClick={() => zoom(1.4)} aria-label="缩小时间线"><Minus size={14} /></button>
          <button onClick={() => pan(-0.25)} aria-label="向前平移"><ChevronLeft size={14} /></button>
          <button onClick={() => pan(0.25)} aria-label="向后平移"><ChevronRight size={14} /></button>
          <button onClick={resetViewport}><RotateCcw size={14} />重置</button>
        </div>
        <span className="timeline-viewport-label">视口 {viewport ? `${formatTimelineTime(viewport.startMs)} – ${formatTimelineTime(viewport.endMs)}` : "—"}</span>
        {windowLoading && <span className="timeline-querying" role="status"><LoaderCircle size={13} className="spin" />查询视口</span>}
      </div>

      {error && <div className="error-banner inline">{error}</div>}
      <div className="timeline-layout">
        <div className="timeline-track-labels" aria-hidden="true">
          <div className="timeline-ruler-spacer" />
          {TIMELINE_TRACKS.map((kind) => {
            const availability = trackAvailability(manifest, kind);
            return <div key={kind} className={availability.state === "available" ? "available" : "unavailable"}><b>{TIMELINE_TRACK_LABELS[kind]}</b><small>{availabilityLabel(availability)}</small></div>;
          })}
        </div>
        <div className="timeline-canvas-wrap" ref={wrapRef}>
          <canvas
            ref={canvasRef}
            className={`timeline-canvas mode-${mode}`}
            width={canvasSize.width * devicePixelRatio}
            height={canvasSize.height * devicePixelRatio}
            style={{ width: canvasSize.width, height: canvasSize.height }}
            aria-label="诊断时间线画布。使用工具栏缩放和平移；画布可拖动或框选。下方提供等价的可访问列表。"
            onPointerDown={onPointerDown}
            onPointerMove={onPointerMove}
            onPointerUp={onPointerUp}
            onPointerCancel={() => setDrag(undefined)}
            onWheel={onWheel}
          />
        </div>
      </div>

      {windowData?.truncated && <p className="diagnostic-warning">当前视口返回的数据已截断；请放大后查看更小时间范围。</p>}
      {manifest.warnings?.map((warning) => <p className="diagnostic-warning" key={warning}>{warning}</p>)}

      <div className="timeline-accessible-list">
        <div><List size={14} /><b>当前视口事件列表</b><span>{visibleItems.length} 项</span></div>
        {availableTracks.length === 0 ? <p>清单中的所有轨道均为未采集、不支持或失败状态。</p> : visibleItems.length === 0 ? <p>{windowLoading ? "正在查询当前视口…" : "当前视口没有事件。轨道可用不表示此时间段内一定有记录。"}</p> : (
          <ul aria-label="当前视口时间线事件">
            {visibleItems.map((item) => <li key={`${item.track}:${item.id}`}><button className={selectedItem?.id === item.id ? "active" : ""} onClick={() => void selectItem(item)}><span className={`timeline-event-dot ${item.track} ${item.severity ?? "normal"}`} /><span>{timelineItemLabel(item)}</span>{item.detail && <small>{item.detail}</small>}</button></li>)}
          </ul>
        )}
      </div>

      <TimelineDetails
        item={selectedItem}
        selection={selection}
        analysis={selectionAnalysis}
        frame={frameDrilldown}
        loading={detailLoading}
        onAnalyze={() => selection && void analyzeSelection(selection)}
      />
    </div>
  );
}

function TimelineState({ title, detail, loading, warning }: { title: string; detail: string; loading?: boolean; warning?: boolean }) {
  return <div className="timeline-state"><div>{loading ? <LoaderCircle className="spin" size={22} /> : warning ? <AlertTriangle size={22} /> : <Search size={22} />}</div><h2>{title}</h2><p>{detail}</p></div>;
}

function TimelineDetails({ item, selection, analysis, frame, loading, onAnalyze }: {
  item?: TimelineItem;
  selection?: TimelineRange;
  analysis?: DiagnosticSelectionAnalysis;
  frame?: FrameDrilldown;
  loading: boolean;
  onAnalyze: () => void;
}) {
  if (!selection && !item) return <div className="timeline-detail-empty"><Crosshair size={18} /><span>选择帧或事件查看详情；使用框选模式分析一个时间段。</span></div>;
  const correlations = frame?.correlations ?? analysis?.correlations ?? [];
  return <section className="timeline-selection-detail" aria-live="polite">
    <div className="timeline-selection-heading">
      <div><span>{item ? TIMELINE_TRACK_LABELS[item.track] : "所选时间段"}</span><h3>{item?.label ?? `${formatTimelineTime(selection?.startMs ?? 0)} – ${formatTimelineTime(selection?.endMs ?? 0)}`}</h3></div>
      {selection && !item && !analysis && <button className="secondary-button" onClick={onAnalyze} disabled={loading}>分析所选时间段</button>}
      {loading && <LoaderCircle className="spin" size={16} />}
    </div>
    {item && <dl className="timeline-item-facts"><div><dt>开始</dt><dd>{formatTimelineTime(item.startMs)}</dd></div><div><dt>时长</dt><dd>{formatTimelineTime(item.durationMs ?? item.endMs - item.startMs)}</dd></div><div><dt>轨道</dt><dd>{TIMELINE_TRACK_LABELS[item.track]}</dd></div>{item.detail && <div><dt>记录</dt><dd>{item.detail}</dd></div>}</dl>}
    {frame && (frame.available ? <div className="timeline-frame-summary"><b>{frame.classification ?? "帧详情"}</b><span>{frame.durationMs === undefined ? "无帧耗时" : formatTimelineTime(frame.durationMs)}{frame.budgetMs === undefined ? "" : ` / 预算 ${formatTimelineTime(frame.budgetMs)}`}</span>{frame.details?.map((detail) => <small key={detail.label}>{detail.label}：{detail.value}</small>)}</div> : <p className="diagnostic-warning">帧详情不可用：{frame.reason ?? "索引中没有此帧证据"}</p>)}
    {analysis?.summary && <p className="timeline-analysis-summary">{analysis.summary}</p>}
    {analysis?.slowFrameCount !== undefined && <p className="timeline-slow-count">所选窗口记录到 <b>{analysis.slowFrameCount}</b> 个慢帧。</p>}
    {(analysis || frame) && <CorrelationList correlations={correlations} />}
    {[...(analysis?.warnings ?? []), ...(frame?.warnings ?? [])].map((warning) => <p className="diagnostic-warning" key={warning}>{warning}</p>)}
  </section>;
}

function CorrelationList({ correlations }: { correlations: DiagnosticCorrelationCandidate[] }) {
  return <div className="timeline-correlations"><div><b>慢帧时间相关候选</b><span>置信度来自时钟质量与时间重叠；不表示导致关系。</span></div>{correlations.length === 0 ? <p>没有可展示的跨轨时间相关候选。缺少时钟映射或相邻事件时不会推导关联。</p> : <ul>{correlations.map((candidate, index) => <li key={candidate.id ?? candidate.itemId ?? `${candidate.label}-${index}`}><span className={`confidence ${candidate.confidence}`}>{confidenceLabel(candidate.confidence)}</span><div><b>{candidate.label}</b><small>{correlationSummary(candidate)}</small>{candidate.reasons.map((reason) => <span key={reason}>{reason}</span>)}</div></li>)}</ul>}</div>;
}

function confidenceLabel(confidence: DiagnosticCorrelationCandidate["confidence"]) {
  return ({ high: "高", medium: "中", low: "低", unavailable: "不可用" })[confidence];
}

function correlationSummary(candidate: DiagnosticCorrelationCandidate) {
  const relation = candidate.relation ? ({ overlaps: "时间重叠", adjacent_before: "相邻（之前）", adjacent_after: "相邻（之后）", contains: "窗口包含候选", contained_by: "候选包含窗口", unavailable: "关系不可用" })[candidate.relation] : "时间候选";
  const overlap = candidate.overlapRatio === undefined ? "" : ` · 重叠 ${(candidate.overlapRatio * 100).toFixed(0)}%`;
  const gap = candidate.gapMs === undefined ? "" : ` · 间隔 ${formatTimelineTime(candidate.gapMs)}`;
  return `${relation}${overlap}${gap}`;
}

function drawTimeline(
  canvas: HTMLCanvasElement | null,
  size: { width: number; height: number },
  viewport: TimelineRange | undefined,
  manifest: DiagnosticManifest | undefined,
  overview: TimelineOverview | undefined,
  items: TimelineItem[],
  selectedId?: number,
  selection?: TimelineRange,
  activeBrush?: TimelineRange,
) {
  if (!canvas) return;
  const ratio = devicePixelRatio || 1;
  const context = canvas.getContext("2d");
  if (!context) return;
  context.setTransform(ratio, 0, 0, ratio, 0, 0);
  context.clearRect(0, 0, size.width, size.height);
  context.fillStyle = "#0b0d12";
  context.fillRect(0, 0, size.width, size.height);
  if (!viewport || !manifest) return;

  context.font = "8px DM Mono, monospace";
  context.textBaseline = "middle";
  const ticks = 6;
  for (let index = 0; index <= ticks; index += 1) {
    const x = index / ticks * size.width;
    const time = viewport.startMs + index / ticks * (viewport.endMs - viewport.startMs);
    context.strokeStyle = "rgba(130, 139, 160, .16)";
    context.beginPath(); context.moveTo(x, RULER_HEIGHT - 6); context.lineTo(x, size.height); context.stroke();
    context.fillStyle = "#7f8798";
    context.fillText(formatTimelineTime(time), Math.min(x + 4, size.width - 54), 11);
  }

  for (let index = 0; index < TIMELINE_TRACKS.length; index += 1) {
    const kind = TIMELINE_TRACKS[index];
    const y = RULER_HEIGHT + index * TRACK_HEIGHT;
    const availability = trackAvailability(manifest, kind);
    context.fillStyle = index % 2 === 0 ? "rgba(255,255,255,.018)" : "rgba(255,255,255,.006)";
    context.fillRect(0, y, size.width, TRACK_HEIGHT);
    context.strokeStyle = "rgba(130, 139, 160, .14)";
    context.beginPath(); context.moveTo(0, y); context.lineTo(size.width, y); context.stroke();
    if (availability.state !== "available") {
      context.fillStyle = "rgba(127, 135, 152, .08)";
      context.fillRect(0, y, size.width, TRACK_HEIGHT);
      context.fillStyle = "#666d7c";
      context.fillText(availabilityLabel(availability), 8, y + TRACK_HEIGHT / 2);
    }
  }

  for (const track of overview?.tracks ?? []) {
    const row = TIMELINE_TRACKS.indexOf(track.kind);
    if (row < 0) continue;
    const max = Math.max(1, ...track.buckets.map((bucket) => bucket.count));
    for (const bucket of track.buckets) {
      if (bucket.count <= 0) continue;
      const x = canvasXAtTime(bucket.startMs, size.width, viewport);
      const endX = canvasXAtTime(bucket.endMs, size.width, viewport);
      const alpha = 0.04 + bucket.count / max * 0.12;
      context.fillStyle = `rgba(124, 92, 255, ${alpha})`;
      context.fillRect(x, RULER_HEIGHT + row * TRACK_HEIGHT + 3, Math.max(1, endX - x), TRACK_HEIGHT - 6);
    }
  }

  for (const item of items) {
    const row = TIMELINE_TRACKS.indexOf(item.track);
    if (row < 0) continue;
    const x = Math.max(0, canvasXAtTime(item.startMs, size.width, viewport));
    const endX = Math.min(size.width, canvasXAtTime(item.endMs, size.width, viewport));
    const width = Math.max(item.track === "js_samples" ? 2 : 3, endX - x);
    const y = RULER_HEIGHT + row * TRACK_HEIGHT + 10;
    context.fillStyle = itemColor(item, item.id === selectedId);
    context.fillRect(x, y, width, TRACK_HEIGHT - 20);
    if (width > 42) {
      context.save(); context.beginPath(); context.rect(x + 3, y, width - 6, TRACK_HEIGHT - 20); context.clip();
      context.fillStyle = item.id === selectedId ? "#fff" : "#dfe3ec";
      context.fillText(item.label, x + 5, y + (TRACK_HEIGHT - 20) / 2);
      context.restore();
    }
  }

  const selectedRange = activeBrush ?? selection;
  if (selectedRange) {
    const left = canvasXAtTime(Math.min(selectedRange.startMs, selectedRange.endMs), size.width, viewport);
    const right = canvasXAtTime(Math.max(selectedRange.startMs, selectedRange.endMs), size.width, viewport);
    context.fillStyle = "rgba(74, 222, 128, .09)";
    context.fillRect(left, RULER_HEIGHT, Math.max(1, right - left), size.height - RULER_HEIGHT);
    context.strokeStyle = "rgba(74, 222, 128, .85)";
    context.strokeRect(left + .5, RULER_HEIGHT + .5, Math.max(1, right - left - 1), size.height - RULER_HEIGHT - 1);
  }
}

function itemColor(item: TimelineItem, selected: boolean) {
  if (selected) return "#7c5cff";
  if (item.severity === "slow" || item.severity === "warning") return "#f4a261";
  if (item.severity === "error") return "#ef6262";
  return ({ iterations: "#5575e7", frames: "#36b37e", react_commits: "#9c6ade", js_samples: "#d9a441", runtime_events: "#4ea5d9" })[item.track];
}
