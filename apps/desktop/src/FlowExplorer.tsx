import { Check, Copy, Crosshair, MousePointer2, Pause, Play, RefreshCw, ScanSearch, ShieldCheck, Smartphone } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { captureDeviceInspector } from "./api";
import type { Device, DeviceInspectorSnapshot, InspectorElement, InspectorSelectorCandidate } from "./types";

interface FlowExplorerProps {
  devices: Device[];
  selectedDeviceId: string;
  activeJobRunning: boolean;
  onSelectDevice: (device: Device) => void;
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
  activeJobRunning,
  onSelectDevice,
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
  const [live, setLive] = useState(false);
  const [error, setError] = useState("");
  const [copiedStrategy, setCopiedStrategy] = useState("");
  const captureInFlight = useRef(false);

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
        <div><p className="eyebrow">INTERACTIVE FLOW EXPLORER · M8.10A</p><h1>看见页面，也看见 Selector</h1></div>
        <div className="top-actions">
          <span className={`status-pill ${activeJobRunning ? "waiting" : "ready"}`}><span className="status-dot" />{activeJobRunning ? "测试运行中 · 同步已暂停" : live ? "低频同步中 · 3 秒" : "本地观察模式"}</span>
          <button className="secondary-button" disabled={!selectedDevice || loading || activeJobRunning} onClick={() => void capture()}>{loading ? <RefreshCw size={16} className="spin" /> : <RefreshCw size={16} />}刷新画面</button>
          <button className="secondary-button" disabled={!selectedDevice || activeJobRunning} onClick={() => setLive((value) => !value)}>{live ? <Pause size={16} /> : <Play size={16} />}{live ? "暂停同步" : "开始同步"}</button>
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

      {activeJobRunning && <div className="explorer-guard"><ShieldCheck size={17} /><div><b>性能测量隔离已生效</b><span>Reactor 不会在任何运行任务期间截屏或读取 UI 树。任务结束后可继续探索。</span></div></div>}
      {error && <div className="error-banner explorer-error">{error}</div>}

      {devices.length === 0 ? (
        <section className="card explorer-empty"><Smartphone size={32} /><h2>启动一个模拟器后开始探索</h2><p>支持 Android Emulator、Android 真机和 iOS Simulator；点击“刷新设备列表”后无需另外安装 Maestro。</p><button className="primary-button" onClick={onRefreshDevices}><RefreshCw size={16} />刷新设备列表</button></section>
      ) : (
        <div className="explorer-grid">
          <section className="card explorer-device-card">
            <div className="card-heading"><div className="heading-icon purple"><MousePointer2 size={18} /></div><div><h2>设备画面</h2><p>点击画面只选择控件；M8.10B 才会录制和执行操作。</p></div>{snapshot && <span className="schema-badge">{new Date(snapshot.capturedAt).toLocaleTimeString()}</span>}</div>
            <div className="device-mirror-stage">
              {snapshot ? (
                <div className="device-mirror" onClick={inspectPoint} title="点击审查此位置的最小 UI 元素">
                  <img src={snapshot.screenshotDataUrl} alt={`${selectedDevice?.name ?? selectedDevice?.id} 当前画面`} draggable={false} />
                  {selectedElement && <span className="element-highlight" style={highlightStyle(selectedElement, snapshot)}><span>{elementName(selectedElement)}</span></span>}
                  {point && <span className="inspection-point" style={{ left: `${(point.x / snapshot.viewportWidth) * 100}%`, top: `${(point.y / snapshot.viewportHeight) * 100}%` }} />}
                </div>
              ) : (
                <div className="mirror-placeholder">{loading ? <RefreshCw size={28} className="spin" /> : <ScanSearch size={32} />}<b>{loading ? "正在同步画面与 UI 树" : "等待首次画面"}</b><span>截图与 UI 树并行获取，不写入测试产物。</span></div>
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
