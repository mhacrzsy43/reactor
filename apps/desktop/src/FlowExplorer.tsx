import { AlertTriangle, Check, Copy, Crosshair, ListPlus, MousePointer2, Pause, Play, RefreshCw, ScanSearch, ShieldCheck, Smartphone, Undo2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { captureDeviceInspector, performExplorerStep } from "./api";
import type { Device, DeviceInspectorSnapshot, FlowStep, InspectorElement, InspectorSelectorCandidate } from "./types";

interface FlowExplorerProps {
  devices: Device[];
  selectedDeviceId: string;
  appId: string;
  activeJobRunning: boolean;
  onSelectDevice: (device: Device) => void;
  onAppIdChange: (appId: string) => void;
  onRefreshDevices: () => void;
}

const stabilityNames = {
  stable: "稳定",
  contextual: "依赖上下文",
  brittle: "脆弱",
} as const;

export function FlowExplorer({
  devices,
  selectedDeviceId,
  appId,
  activeJobRunning,
  onSelectDevice,
  onAppIdChange,
  onRefreshDevices,
}: FlowExplorerProps) {
  const selectedDevice = useMemo(
    () => devices.find((device) => device.id === selectedDeviceId) ?? devices[0],
    [devices, selectedDeviceId],
  );
  const [snapshot, setSnapshot] = useState<DeviceInspectorSnapshot>();
  const [selectedElementKey, setSelectedElementKey] = useState<string>();
  const [point, setPoint] = useState<{ x: number; y: number }>();
  const [loading, setLoading] = useState(false);
  const [interacting, setInteracting] = useState(false);
  const [interactingLabel, setInteractingLabel] = useState("");
  const [live, setLive] = useState(false);
  const [mode, setMode] = useState<"inspect" | "record">("inspect");
  const [recordedSteps, setRecordedSteps] = useState<FlowStep[]>([]);
  const [pendingDanger, setPendingDanger] = useState<InspectorElement>();
  const [error, setError] = useState("");
  const [copiedStrategy, setCopiedStrategy] = useState("");
  const captureInFlight = useRef(false);
  const interactionInFlight = useRef(false);

  const capture = useCallback(async () => {
    if (!selectedDevice || activeJobRunning || captureInFlight.current) return;
    captureInFlight.current = true;
    setLoading(true);
    setError("");
    try {
      const next = await captureDeviceInspector({
        platform: selectedDevice.platform === "ios" ? "ios" : "android",
        deviceId: selectedDevice.id,
      });
      setSnapshot(next);
      setSelectedElementKey((current) => current && next.elements.some((element) => element.key === current) ? current : undefined);
    } catch (reason) {
      setError(String(reason));
      setLive(false);
    } finally {
      setLoading(false);
      captureInFlight.current = false;
    }
  }, [activeJobRunning, selectedDevice]);

  useEffect(() => {
    setSnapshot(undefined);
    setSelectedElementKey(undefined);
    setPoint(undefined);
    setError("");
    setLive(false);
    if (selectedDevice && !activeJobRunning) void capture();
  }, [activeJobRunning, capture, selectedDevice?.id, selectedDevice?.platform]);

  useEffect(() => {
    if (!live || activeJobRunning) return undefined;
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") void capture();
    }, 3_000);
    return () => window.clearInterval(timer);
  }, [activeJobRunning, capture, live]);

  const selectedElement = snapshot?.elements.find((element) => element.key === selectedElementKey);

  function inspectPoint(event: React.MouseEvent<HTMLDivElement>) {
    if (!snapshot) return;
    const rect = event.currentTarget.getBoundingClientRect();
    const x = ((event.clientX - rect.left) / rect.width) * snapshot.viewportWidth;
    const y = ((event.clientY - rect.top) / rect.height) * snapshot.viewportHeight;
    const hit = hitTest(snapshot.elements, x, y);
    setPoint({ x, y });
    setSelectedElementKey(hit?.key);
    setError("");
    if (mode === "record" && hit) {
      if (!hit.clickable) {
        setError("此位置只有结构元素，Reactor 不会自动执行脆弱坐标；请审查后显式选择坐标降级。");
        return;
      }
      if (isDangerousElement(hit)) {
        setPendingDanger(hit);
      } else {
        void recordTap(hit);
      }
    }
  }

  async function recordTap(element: InspectorElement) {
    const candidate = element.candidates[0];
    if (!selectedDevice || !candidate || !appId.trim() || activeJobRunning || interactionInFlight.current) return;
    const step: FlowStep = { action: "tap", target: candidate.selector };
    await executeRecordedStep(step, `点击 ${elementName(element)}`);
  }

  async function recordSwipe(direction: "up" | "down") {
    const step: FlowStep = { action: "swipe", direction, duration_ms: 500 };
    await executeRecordedStep(step, direction === "up" ? "向上滚动" : "向下滚动");
  }

  async function executeRecordedStep(step: FlowStep, label: string) {
    if (!selectedDevice || !appId.trim() || activeJobRunning || interactionInFlight.current) return;
    interactionInFlight.current = true;
    setLive(false);
    setInteracting(true);
    setInteractingLabel(label);
    setError("");
    setPendingDanger(undefined);
    try {
      const next = await performExplorerStep({
        platform: selectedDevice.platform === "ios" ? "ios" : "android",
        deviceId: selectedDevice.id,
        appId: appId.trim(),
        step,
      });
      setRecordedSteps((current) => [...current, step]);
      setSnapshot(next);
      setSelectedElementKey(undefined);
      setPoint(undefined);
    } catch (reason) {
      setError(`交互执行失败，步骤未加入 Flow：${String(reason)}`);
    } finally {
      setInteracting(false);
      setInteractingLabel("");
      interactionInFlight.current = false;
    }
  }

  function handleMirrorWheel(event: React.WheelEvent<HTMLDivElement>) {
    event.preventDefault();
    event.stopPropagation();
    if (Math.abs(event.deltaY) < 8 || interactionInFlight.current) return;
    if (mode !== "record") {
      setError("镜像内滚动已被 Reactor 拦截；切换到录制/交互模式后会转换为设备滑动。");
      return;
    }
    void recordSwipe(event.deltaY > 0 ? "up" : "down");
  }

  async function copySelector(candidate: InspectorSelectorCandidate) {
    try {
      await navigator.clipboard.writeText(JSON.stringify(candidate.selector, null, 2));
      setCopiedStrategy(candidate.strategy);
      window.setTimeout(() => setCopiedStrategy(""), 1_500);
    } catch (reason) {
      setError(`复制 Selector 失败：${String(reason)}`);
    }
  }

  return (
    <>
      <header className="topbar">
        <div><p className="eyebrow">INTERACTIVE FLOW EXPLORER · M8.10B</p><h1>看见页面，逐步录成 Flow</h1></div>
        <div className="top-actions">
          <span className={`status-pill ${activeJobRunning ? "waiting" : "ready"}`}><span className="status-dot" />{activeJobRunning ? "测试运行中 · 同步已暂停" : interacting ? "正在执行并等待页面稳定" : live ? "低频同步中 · 3 秒" : mode === "record" ? "录制/交互模式" : "审查模式"}</span>
          <button className="secondary-button" disabled={!selectedDevice || loading || interacting || activeJobRunning} onClick={() => void capture()}>{loading ? <RefreshCw size={16} className="spin" /> : <RefreshCw size={16} />}刷新画面</button>
          <button className="secondary-button" disabled={!selectedDevice || interacting || activeJobRunning} onClick={() => setLive((value) => !value)}>{live ? <Pause size={16} /> : <Play size={16} />}{live ? "暂停同步" : "开始同步"}</button>
        </div>
      </header>

      <section className="explorer-toolbar card">
        <label>
          <span>测试目标</span>
          <select
            value={selectedDevice ? `${selectedDevice.platform}:${selectedDevice.id}` : ""}
            onChange={(event) => {
              const device = devices.find((candidate) => `${candidate.platform}:${candidate.id}` === event.target.value);
              if (device) onSelectDevice(device);
            }}
          >
            {devices.length === 0 && <option value="">尚未发现设备</option>}
            {devices.map((device) => <option key={`${device.platform}:${device.id}`} value={`${device.platform}:${device.id}`}>{device.platform === "ios" ? "iOS" : "Android"} · {device.name ?? device.id} · {device.physical ? "真机" : "模拟器"}</option>)}
          </select>
        </label>
        <div className="explorer-toolbar-summary"><Smartphone size={16} /><div><b>{selectedDevice?.id ?? "等待连接"}</b><span>{snapshot ? `${snapshot.screenshotWidth} × ${snapshot.screenshotHeight} PNG · ${snapshot.elements.length} 个 UI 元素` : "画面和 UI 树只保存在当前内存中"}</span></div></div>
        <button className="secondary-button" onClick={onRefreshDevices}><RefreshCw size={15} />刷新设备列表</button>
      </section>

      <section className="recording-console card">
        <div className="recording-mode" role="group" aria-label="Flow Explorer 模式">
          <button className={mode === "inspect" ? "active" : ""} onClick={() => { setMode("inspect"); setPendingDanger(undefined); }}>审查模式<span>只看 Selector</span></button>
          <button className={mode === "record" ? "active" : ""} onClick={() => { setMode("record"); setLive(false); }}>录制/交互模式<span>点击后进入下一页</span></button>
        </div>
        <label className="recording-app-id"><span>当前 App 包名 / Bundle ID</span><input value={appId} onChange={(event) => onAppIdChange(event.target.value)} placeholder="com.example.app" /></label>
        <div className="recording-progress"><ListPlus size={17} /><div><b>{recordedSteps.length} 个已录制步骤</b><span>{mode === "record" ? "点击画面后 Reactor 使用最佳语义 Selector 真实执行，并等待下一页面稳定。" : "切换到录制/交互模式后才会操作设备。"}</span></div></div>
        <button className="secondary-button" disabled={recordedSteps.length === 0 || interacting} onClick={() => setRecordedSteps((steps) => steps.slice(0, -1))}><Undo2 size={15} />移除最后一步</button>
      </section>

      {recordedSteps.length > 0 && <section className="recorded-step-strip" aria-label="已录制步骤">{recordedSteps.map((step, index) => <div key={`${step.action}-${index}`}><span>{index + 1}</span><b>{step.action === "tap" ? "点击" : step.action}</b><code>{step.action === "tap" ? selectorLabel(step.target) : ""}</code></div>)}</section>}

      {pendingDanger && <div className="explorer-guard danger"><AlertTriangle size={18} /><div><b>检测到潜在敏感操作：{elementName(pendingDanger)}</b><span>当前安全阶段不会执行或写入步骤；敏感操作确认凭证接通后才会开放“确认并继续”。</span></div><button className="secondary-button" onClick={() => setPendingDanger(undefined)}>取消</button></div>}

      {activeJobRunning && <div className="explorer-guard"><ShieldCheck size={17} /><div><b>性能测量隔离已生效</b><span>Reactor 不会在任何运行任务期间截屏或读取 UI 树。任务结束后可继续探索。</span></div></div>}
      {error && <div className="error-banner explorer-error">{error}</div>}

      {devices.length === 0 ? (
        <section className="card explorer-empty"><Smartphone size={32} /><h2>启动一个模拟器后开始探索</h2><p>支持 Android Emulator、Android 真机和 iOS Simulator；点击“刷新设备列表”后无需另外安装 Maestro。</p><button className="primary-button" onClick={onRefreshDevices}><RefreshCw size={16} />刷新设备列表</button></section>
      ) : (
        <div className="explorer-grid">
          <section className="card explorer-device-card">
            <div className="card-heading"><div className="heading-icon purple"><MousePointer2 size={18} /></div><div><h2>设备画面</h2><p>{mode === "record" ? "点击控件会真实执行、追加步骤并刷新下一页面。" : "点击只审查控件，不会改变 App。"}</p></div>{snapshot && <span className="schema-badge">{new Date(snapshot.capturedAt).toLocaleTimeString()}</span>}</div>
            <div className="device-mirror-stage">
              {snapshot ? (
                <div className={`device-mirror ${mode}`} onClick={inspectPoint} onWheel={handleMirrorWheel} title={mode === "record" ? "点击并录制；滚轮/触控板转换为设备滑动" : "点击审查；镜像内滚动不会滚动 Reactor"}>
                  <img src={snapshot.screenshotDataUrl} alt={`${selectedDevice?.name ?? selectedDevice?.id} 当前画面`} draggable={false} />
                  {selectedElement && <span className="element-highlight" style={highlightStyle(selectedElement, snapshot)}><span>{elementName(selectedElement)}</span></span>}
                  {point && <span className="inspection-point" style={{ left: `${(point.x / snapshot.viewportWidth) * 100}%`, top: `${(point.y / snapshot.viewportHeight) * 100}%` }} />}
                  {interacting && <span className="mirror-interaction-overlay"><RefreshCw size={22} className="spin" /><b>{interactingLabel}</b><small>正在操作设备并等待下一页面稳定</small></span>}
                </div>
              ) : (
                <div className="mirror-placeholder">{loading || interacting ? <RefreshCw size={28} className="spin" /> : <ScanSearch size={32} />}<b>{interacting ? "正在执行步骤并等待下一页面" : loading ? "正在同步画面与 UI 树" : "等待首次画面"}</b><span>截图与 UI 树并行获取，不写入测试产物。</span></div>
              )}
            </div>
            {snapshot?.warnings.map((warning) => <div className="explorer-warning" key={warning}>{warning}</div>)}
          </section>

          <aside className="card selector-inspector-card">
            <div className="card-heading"><div className="heading-icon green"><Crosshair size={18} /></div><div><h2>Selector Inspector</h2><p>优先语义定位，坐标仅作显式降级。</p></div></div>
            {selectedElement ? (
              <>
                <div className="element-summary">
                  <span className={`element-state ${selectedElement.clickable ? "clickable" : ""}`}>{selectedElement.clickable ? "可交互" : "结构元素"}</span>
                  <h3>{elementName(selectedElement)}</h3>
                  <code>{selectedElement.resourceId ?? selectedElement.key}</code>
                  <div><span>位置</span><b>{formatBounds(selectedElement)}</b></div>
                  <div><span>层级</span><b>Depth {selectedElement.depth}</b></div>
                  <div><span>状态</span><b>{selectedElement.enabled ? "Enabled" : "Disabled"}</b></div>
                </div>
                <button className="primary-button inspector-execute-button" disabled={!selectedElement.clickable || interacting || activeJobRunning || !appId.trim()} onClick={() => { setMode("record"); void recordTap(selectedElement); }}><Play size={15} />在设备上点击并继续</button>
                <div className="selector-candidates">
                  <div className="selector-list-heading"><b>候选 Selector</b><span>按稳定性排序</span></div>
                  {selectedElement.candidates.map((candidate) => (
                    <article className={`selector-candidate ${candidate.stability}`} key={`${candidate.strategy}:${candidate.label}`}>
                      <div className="candidate-score"><strong>{candidate.score}</strong><span>/ 100</span></div>
                      <div className="candidate-main"><div><b>{candidate.strategy}</b><span className={`stability-badge ${candidate.stability}`}>{stabilityNames[candidate.stability]}</span></div><code>{candidate.label}</code><p>{candidate.reason}</p></div>
                      <button className="icon-button small" title="复制 Selector JSON" onClick={() => void copySelector(candidate)}>{copiedStrategy === candidate.strategy ? <Check size={14} /> : <Copy size={14} />}</button>
                    </article>
                  ))}
                </div>
              </>
            ) : (
              <div className="selector-empty"><ScanSearch size={28} /><h3>点击设备画面中的控件</h3><p>Reactor 会在 UI 树中命中面积最小的元素，并解释每个 Selector 为什么稳定或脆弱。</p></div>
            )}
          </aside>
        </div>
      )}
    </>
  );
}

function hitTest(elements: InspectorElement[], x: number, y: number): InspectorElement | undefined {
  const matching = elements.filter((element) => element.bounds.width > 0 && element.bounds.height > 0 && x >= element.bounds.x && y >= element.bounds.y && x <= element.bounds.x + element.bounds.width && y <= element.bounds.y + element.bounds.height);
  const interactive = matching.filter((element) => element.clickable && element.enabled);
  return (interactive.length > 0 ? interactive : matching)
    .sort((left, right) => left.bounds.width * left.bounds.height - right.bounds.width * right.bounds.height)[0];
}

function elementName(element: InspectorElement): string {
  return element.text ?? element.accessibilityText ?? element.resourceId?.split("/").pop() ?? "未命名元素";
}

function isDangerousElement(element: InspectorElement): boolean {
  const value = [element.text, element.accessibilityText, element.resourceId].filter(Boolean).join(" ").toLowerCase();
  return ["delete", "remove account", "pay", "purchase", "buy", "checkout", "transfer", "submit", "authorize", "删除", "支付", "购买", "下单", "转账", "授权", "提交", "注销", "退出登录"].some((keyword) => value.includes(keyword));
}

function selectorLabel(selector: InspectorSelectorCandidate["selector"]): string {
  return selector.accessibilityId ?? selector.semanticId ?? selector.text ?? (selector.coordinate ? `${Math.round(selector.coordinate.x)},${Math.round(selector.coordinate.y)}` : "未知 Selector");
}

function formatBounds(element: InspectorElement): string {
  const bounds = element.bounds;
  return `${Math.round(bounds.x)}, ${Math.round(bounds.y)} · ${Math.round(bounds.width)} × ${Math.round(bounds.height)}`;
}

function highlightStyle(element: InspectorElement, snapshot: DeviceInspectorSnapshot) {
  return {
    left: `${(element.bounds.x / snapshot.viewportWidth) * 100}%`,
    top: `${(element.bounds.y / snapshot.viewportHeight) * 100}%`,
    width: `${(element.bounds.width / snapshot.viewportWidth) * 100}%`,
    height: `${(element.bounds.height / snapshot.viewportHeight) * 100}%`,
  };
}
