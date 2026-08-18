import { AlertTriangle, ArrowDown, ArrowUp, Braces, Check, Code2, Copy, Crosshair, GitBranch, ListPlus, MousePointer2, Pause, Play, RefreshCw, RotateCcw, ScanSearch, ShieldCheck, Sparkles, Smartphone, Trash2, Undo2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { captureDeviceInspector, compileFlowPreview, getFlowSecretStatus, performExplorerStep, probeFlow, replayRecordedFlow, saveFlowSecret } from "./api";
import type { CompiledFlow, Device, DeviceInspectorSnapshot, Flow, FlowStep, InputValue, InspectorElement, InspectorSelectorCandidate } from "./types";

interface ExplorerGraphNode {
  id: string;
  label: string;
  elementCount: number;
  capturedAt: string;
}

interface ExplorerGraphTransition {
  id: string;
  from: string;
  to: string;
  action: string;
}

interface ExplorerSuggestion {
  step: FlowStep;
  label: string;
  provider: string;
  model: string;
  knownTarget: boolean;
  dangerous: boolean;
  coordinateFallback: boolean;
  executionPoint?: { x: number; y: number };
}

interface FlowExplorerProps {
  devices: Device[];
  selectedDeviceId: string;
  appId: string;
  goal: string;
  ai: {
    provider: "offline" | "local" | "codex" | "claude" | "cloud";
    endpoint: string;
    model: string;
    apiKey?: string;
    saveApiKey: boolean;
    useSavedApiKey: boolean;
    cliExecutable?: string;
  };
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
  goal,
  ai,
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
  const [measurementStart, setMeasurementStart] = useState<number>();
  const [teardownStart, setTeardownStart] = useState(0);
  const [flowView, setFlowView] = useState<"steps" | "json" | "yaml">("steps");
  const [jsonDraft, setJsonDraft] = useState("");
  const [jsonDirty, setJsonDirty] = useState(false);
  const [compiledFlow, setCompiledFlow] = useState<CompiledFlow>();
  const [editorError, setEditorError] = useState("");
  const [editorUndo, setEditorUndo] = useState<{ steps: FlowStep[]; measurementStart?: number; teardownStart: number }>();
  const [replaying, setReplaying] = useState(false);
  const [promptValues, setPromptValues] = useState<Record<string, string>>({});
  const [graphNodes, setGraphNodes] = useState<ExplorerGraphNode[]>([]);
  const [graphTransitions, setGraphTransitions] = useState<ExplorerGraphTransition[]>([]);
  const [suggesting, setSuggesting] = useState(false);
  const [suggestion, setSuggestion] = useState<ExplorerSuggestion>();
  const [suggestionConfirmed, setSuggestionConfirmed] = useState(false);
  const [pendingDanger, setPendingDanger] = useState<InspectorElement>();
  const [error, setError] = useState("");
  const [copiedStrategy, setCopiedStrategy] = useState("");
  const [inputKind, setInputKind] = useState<"literal" | "variableRef" | "secretRef" | "promptRef" | "totpRef">("literal");
  const [inputValue, setInputValue] = useState("");
  const [inputReference, setInputReference] = useState("");
  const [secretValue, setSecretValue] = useState("");
  const [clearBeforeInput, setClearBeforeInput] = useState(true);
  const [inputBusy, setInputBusy] = useState(false);
  const [secretStored, setSecretStored] = useState(false);
  const captureInFlight = useRef(false);
  const interactionInFlight = useRef(false);
  const mirrorRef = useRef<HTMLDivElement>(null);
  const lastWheelGestureAt = useRef(0);
  const teardownStartRef = useRef(0);
  const currentGraphStateRef = useRef<string | undefined>(undefined);
  const pendingGraphStepRef = useRef<FlowStep | undefined>(undefined);

  useEffect(() => {
    teardownStartRef.current = teardownStart;
  }, [teardownStart]);

  useEffect(() => {
    setGraphNodes([]);
    setGraphTransitions([]);
    setSuggestion(undefined);
    currentGraphStateRef.current = undefined;
    pendingGraphStepRef.current = undefined;
  }, [selectedDeviceId, appId]);

  function observeSnapshot(next: DeviceInspectorSnapshot, step?: FlowStep) {
    if (next.elements.length === 0) return;
    const node = graphNode(next);
    const from = currentGraphStateRef.current;
    setGraphNodes((nodes) => nodes.some((candidate) => candidate.id === node.id) ? nodes : [...nodes, node]);
    if (step && from) {
      const transition: ExplorerGraphTransition = {
        id: `${from}:${node.id}:${flowStepName(step)}`,
        from,
        to: node.id,
        action: flowStepName(step),
      };
      setGraphTransitions((transitions) => transitions.some((candidate) => candidate.id === transition.id) ? transitions : [...transitions, transition]);
    }
    currentGraphStateRef.current = node.id;
  }

  const explorerFlow = useMemo<Flow>(() => {
    const setupEnd = measurementStart ?? teardownStart;
    return {
      schemaVersion: 1,
      id: "interactive-recording",
      name: "Interactive recording",
      appId: appId.trim(),
      platform: selectedDevice?.platform === "ios" ? "ios" : "android",
      setup: recordedSteps.slice(0, setupEnd),
      measured: measurementStart === undefined ? [] : recordedSteps.slice(measurementStart, teardownStart),
      teardown: recordedSteps.slice(teardownStart),
    };
  }, [appId, measurementStart, recordedSteps, selectedDevice?.platform, teardownStart]);

  const promptReferences = useMemo(() => collectPromptReferences(explorerFlow), [explorerFlow]);

  useEffect(() => {
    if (!jsonDirty) setJsonDraft(JSON.stringify(explorerFlow, null, 2));
    if (explorerFlow.measured.length === 0 || !explorerFlow.appId) {
      setCompiledFlow(undefined);
      return;
    }
    let cancelled = false;
    void compileFlowPreview(explorerFlow).then((compiled) => {
      if (!cancelled) {
        setCompiledFlow(compiled);
        setEditorError("");
      }
    }).catch((reason) => {
      if (!cancelled) {
        setCompiledFlow(undefined);
        setEditorError(`Flow 校验失败：${String(reason)}`);
      }
    });
    return () => { cancelled = true; };
  }, [explorerFlow, jsonDirty]);

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
      const pendingStep = pendingGraphStepRef.current;
      observeSnapshot(next, pendingStep);
      if (next.elements.length > 0) pendingGraphStepRef.current = undefined;
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

  useEffect(() => {
    if (!selectedElement?.editable) return;
    setInputKind(selectedElement.password ? "secretRef" : "literal");
    setInputValue("");
    setInputReference(selectedElement.password ? "test-account.password" : "");
    setSecretValue("");
    setSecretStored(false);
  }, [selectedElement?.key, selectedElement?.editable, selectedElement?.password]);

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
      if (hit.editable) {
        setError("已选中输入框；请在右侧选择普通文本、变量、Secret、验证码或 TOTP 后执行。");
        return;
      }
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
    if (!selectedDevice) {
      setError("尚未选择可操作的设备。");
      return;
    }
    if (!candidate) {
      setError("当前元素没有可执行的 Selector，步骤未加入 Flow。");
      return;
    }
    if (!appId.trim()) {
      setError("请先填写当前 App 包名 / Bundle ID，再执行并录制这个步骤。");
      return;
    }
    if (activeJobRunning) {
      setError("性能任务运行期间不能操作设备，请等待任务结束。");
      return;
    }
    if (interactionInFlight.current) return;
    const step: FlowStep = { action: "tap", target: candidate.selector };
    await executeRecordedStep(step, `点击 ${elementName(element)}`, {
      x: element.bounds.x + element.bounds.width / 2,
      y: element.bounds.y + element.bounds.height / 2,
    });
  }

  function beginRecording() {
    setMode("record");
    setLive(false);
    if (recordedSteps.length === 0) {
      const initial: FlowStep = { action: "launch_app" };
      setRecordedSteps([initial]);
      teardownStartRef.current = 1;
      setTeardownStart(1);
      setJsonDirty(false);
    }
  }

  async function recordSwipe(direction: "up" | "down") {
    const step: FlowStep = { action: "swipe", direction, duration_ms: 500 };
    await executeRecordedStep(step, direction === "up" ? "向上滚动" : "向下滚动");
  }

  async function recordInput(element: InspectorElement) {
    const candidate = element.candidates[0];
    if (!candidate) {
      setError("当前输入框没有可执行的 Selector。");
      return;
    }
    setInputBusy(true);
    setError("");
    try {
      let value: InputValue;
      let runtimeInput: string | undefined;
      if (inputKind === "literal") {
        if (element.password) throw new Error("密码输入框不能把明文写入 Flow，请选择系统 Secret、变量或交互输入");
        if (!inputValue) throw new Error("普通测试文本不能为空");
        value = inputValue;
      } else {
        const reference = inputReference.trim();
        if (!reference) throw new Error("引用名称不能为空");
        if (inputKind === "variableRef") value = { variableRef: reference };
        else if (inputKind === "promptRef") {
          if (!inputValue) throw new Error("本次交互验证码不能为空");
          value = { promptRef: reference };
          runtimeInput = inputValue;
        } else {
          if (secretValue) {
            await saveFlowSecret(reference, secretValue);
            setSecretValue("");
            setSecretStored(true);
          } else {
            const status = await getFlowSecretStatus(reference);
            if (!status.stored) throw new Error("该引用尚未保存到系统凭据库");
            setSecretStored(true);
          }
          value = inputKind === "secretRef" ? { secretRef: reference } : { totpRef: reference };
        }
      }
      const step: FlowStep = {
        action: "input_text",
        target: candidate.selector,
        value,
        clearBefore: clearBeforeInput,
      };
      await executeRecordedStep(
        step,
        `输入 ${inputValueLabel(value)}`,
        {
          x: element.bounds.x + element.bounds.width / 2,
          y: element.bounds.y + element.bounds.height / 2,
        },
        runtimeInput,
      );
      if (inputKind === "literal" || inputKind === "promptRef") setInputValue("");
    } catch (reason) {
      setError(`输入步骤未执行：${String(reason)}`);
    } finally {
      setInputBusy(false);
    }
  }

  async function executeRecordedStep(step: FlowStep, label: string, executionPoint?: { x: number; y: number }, runtimeInput?: string) {
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
        executionPoint,
        viewportWidth: snapshot?.viewportWidth,
        viewportHeight: snapshot?.viewportHeight,
        runtimeInput,
      });
      setRecordedSteps((current) => {
        const base: FlowStep[] = current.length === 0 ? [{ action: "launch_app" }] : current;
        const insertAt = current.length === 0 ? 1 : Math.min(teardownStartRef.current, base.length);
        const next = [...base.slice(0, insertAt), step, ...base.slice(insertAt)];
        teardownStartRef.current = insertAt + 1;
        setTeardownStart(insertAt + 1);
        return next;
      });
      setSnapshot(next);
      if (next.elements.length > 0) observeSnapshot(next, step);
      else pendingGraphStepRef.current = step;
      setSelectedElementKey(undefined);
      setPoint(undefined);
      if (next.platform === "android" && next.elements.length === 0) {
        window.setTimeout(() => void capture(), 0);
      }
    } catch (reason) {
      setError(`交互执行失败，步骤未加入 Flow：${String(reason)}`);
    } finally {
      setInteracting(false);
      setInteractingLabel("");
      interactionInFlight.current = false;
    }
  }

  function rememberEditorState() {
    setEditorUndo({ steps: recordedSteps, measurementStart, teardownStart });
  }

  function removeRecordedStep(index: number) {
    rememberEditorState();
    setRecordedSteps((steps) => steps.filter((_, stepIndex) => stepIndex !== index));
    if (measurementStart !== undefined && index < measurementStart) {
      setMeasurementStart(Math.max(0, measurementStart - 1));
    }
    if (index < teardownStart) setTeardownStart(Math.max(0, teardownStart - 1));
  }

  function moveRecordedStep(index: number, direction: -1 | 1) {
    const destination = index + direction;
    if (destination < 0 || destination >= recordedSteps.length) return;
    const section = stepSection(index, measurementStart, teardownStart);
    if (stepSection(destination, measurementStart, teardownStart) !== section) {
      setEditorError("跨 setup / measured / teardown 移动请使用 JSON 编辑，避免意外改变测量边界。");
      return;
    }
    rememberEditorState();
    setRecordedSteps((steps) => {
      const next = [...steps];
      [next[index], next[destination]] = [next[destination], next[index]];
      return next;
    });
  }

  function undoEditorChange() {
    if (!editorUndo) return;
    setRecordedSteps(editorUndo.steps);
    setMeasurementStart(editorUndo.measurementStart);
    setTeardownStart(editorUndo.teardownStart);
    setEditorUndo(undefined);
    setJsonDirty(false);
    setEditorError("");
  }

  async function applyJsonDraft() {
    try {
      const parsed = JSON.parse(jsonDraft) as Flow;
      if (parsed.appId !== appId.trim()) throw new Error("JSON appId 必须与当前目标 App 一致");
      if (parsed.platform !== explorerFlow.platform) throw new Error("JSON platform 必须与当前设备平台一致");
      const compiled = await compileFlowPreview(parsed);
      rememberEditorState();
      setRecordedSteps([...parsed.setup, ...parsed.measured, ...parsed.teardown]);
      setMeasurementStart(parsed.setup.length);
      setTeardownStart(parsed.setup.length + parsed.measured.length);
      setCompiledFlow(compiled);
      setJsonDirty(false);
      setEditorError("");
    } catch (reason) {
      setEditorError(`无法应用 JSON：${cleanError(reason)}`);
    }
  }

  async function replayWholeFlow() {
    if (!selectedDevice || !compiledFlow) return;
    const missingPrompt = promptReferences.find((reference) => !promptValues[reference]);
    if (missingPrompt) {
      setEditorError(`整体回放前请输入本次验证码：${missingPrompt}`);
      return;
    }
    setReplaying(true);
    setLive(false);
    setEditorError("");
    try {
      const next = await replayRecordedFlow({
        platform: explorerFlow.platform,
        deviceId: selectedDevice.id,
        flow: explorerFlow,
        promptValues,
      });
      setSnapshot(next);
      observeSnapshot(next);
      setPromptValues({});
      setSelectedElementKey(undefined);
    } catch (reason) {
      setEditorError(`整体回放失败：${cleanError(reason)}`);
    } finally {
      setReplaying(false);
    }
  }

  async function replayOneStep(step: FlowStep) {
    if (!selectedDevice || activeJobRunning || replaying) return;
    const promptReference = step.action === "input_text" && typeof step.value !== "string" && "promptRef" in step.value ? step.value.promptRef : undefined;
    const runtimeInput = promptReference ? promptValues[promptReference] : undefined;
    if (promptReference && !runtimeInput) {
      setEditorError(`逐步回放前请输入本次验证码：${promptReference}`);
      return;
    }
    setReplaying(true);
    setEditorError("");
    try {
      const next = await performExplorerStep({
        platform: explorerFlow.platform,
        deviceId: selectedDevice.id,
        appId: appId.trim(),
        step,
        runtimeInput,
      });
      setSnapshot(next);
      observeSnapshot(next, step);
      if (promptReference) setPromptValues((values) => ({ ...values, [promptReference]: "" }));
    } catch (reason) {
      setEditorError(`逐步回放失败：${cleanError(reason)}`);
    } finally {
      setReplaying(false);
    }
  }

  async function generateNextSuggestion() {
    if (!snapshot || !appId.trim()) return;
    setSuggesting(true);
    setSuggestion(undefined);
    setSuggestionConfirmed(false);
    setEditorError("");
    try {
      if (ai.provider === "offline") {
        const offline = offlineNextSuggestion(snapshot, goal);
        if (!offline) throw new Error("当前页面没有可安全建议的语义控件");
        setSuggestion(offline);
        return;
      }
      const generated = await probeFlow({
        intent: goal,
        appId: appId.trim(),
        platform: explorerFlow.platform,
        uiTree: explorerAiContext(snapshot),
        provider: ai.provider,
        endpoint: ai.endpoint,
        model: ai.model,
        apiKey: ai.apiKey,
        saveApiKey: ai.saveApiKey,
        useSavedApiKey: ai.useSavedApiKey,
        cliExecutable: ai.cliExecutable,
      });
      const steps = [...generated.flow.setup, ...generated.flow.measured, ...generated.flow.teardown];
      const step = steps.find((candidate) => candidate.action === "tap" || candidate.action === "swipe");
      if (!step) throw new Error("Provider 没有返回可审查的下一步动作");
      const target = step.action === "tap" ? step.target : undefined;
      const targetElement = target ? snapshot.elements.find((element) => selectorsOverlap(element, target)) : undefined;
      const knownTarget = !target || Boolean(targetElement) || Boolean(target.coordinate);
      setSuggestion({
        step,
        label: flowStepDetail(step) || flowStepName(step),
        provider: generated.provider,
        model: generated.model,
        knownTarget,
        dangerous: target ? isDangerousSelector(target) || Boolean(targetElement && isDangerousElement(targetElement)) : false,
        coordinateFallback: Boolean(target?.coordinate),
        executionPoint: targetElement
          ? { x: targetElement.bounds.x + targetElement.bounds.width / 2, y: targetElement.bounds.y + targetElement.bounds.height / 2 }
          : target?.coordinate,
      });
    } catch (reason) {
      setEditorError(`无法生成下一步建议：${cleanError(reason)}`);
    } finally {
      setSuggesting(false);
    }
  }

  async function executeSuggestion() {
    if (!suggestion || suggestion.dangerous) return;
    if (!suggestion.knownTarget) {
      setEditorError("AI 建议的目标不在当前真实 UI 树中，Reactor 不会盲目执行；请先在镜像中审查目标。");
      return;
    }
    if (suggestion.coordinateFallback && !suggestionConfirmed) {
      setSuggestionConfirmed(true);
      return;
    }
    await executeRecordedStep(suggestion.step, `AI 建议：${suggestion.label}`, suggestion.executionPoint);
    setSuggestion(undefined);
    setSuggestionConfirmed(false);
  }

  async function copyFlowSource() {
    const source = flowView === "yaml" && compiledFlow ? maestroPreview(compiledFlow) : flowView === "json" ? jsonDraft : JSON.stringify(recordedSteps, null, 2);
    try {
      await navigator.clipboard.writeText(source);
      setCopiedStrategy("flow-source");
      window.setTimeout(() => setCopiedStrategy(""), 1_500);
    } catch (reason) {
      setEditorError(`复制失败：${cleanError(reason)}`);
    }
  }

  const handleMirrorWheel = useCallback((event: WheelEvent) => {
    event.preventDefault();
    event.stopPropagation();
    event.stopImmediatePropagation();
    if (Math.abs(event.deltaY) < 8 || interactionInFlight.current) return;
    const now = Date.now();
    if (now - lastWheelGestureAt.current < 1_200) return;
    lastWheelGestureAt.current = now;
    if (mode !== "record") {
      setError("镜像内滚动已被 Reactor 拦截；切换到录制/交互模式后会转换为设备滑动。");
      return;
    }
    if (!appId.trim()) {
      setError("请先填写当前 App 包名 / Bundle ID，再把滚动录制为设备滑动。");
      return;
    }
    void recordSwipe(event.deltaY > 0 ? "up" : "down");
  }, [appId, mode]);

  useEffect(() => {
    const interceptMirrorWheel = (event: WheelEvent) => {
      const mirror = mirrorRef.current;
      if (mirror && event.target instanceof Node && mirror.contains(event.target)) handleMirrorWheel(event);
    };
    window.addEventListener("wheel", interceptMirrorWheel, { capture: true, passive: false });
    return () => window.removeEventListener("wheel", interceptMirrorWheel, { capture: true });
  }, [handleMirrorWheel, snapshot]);

  useEffect(() => () => document.body.classList.remove("mirror-gesture-lock"), []);

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
          <button className={mode === "record" ? "active" : ""} onClick={beginRecording}>录制/交互模式<span>从启动 App 开始录制</span></button>
        </div>
        <label className="recording-app-id"><span>当前 App 包名 / Bundle ID</span><input value={appId} onChange={(event) => onAppIdChange(event.target.value)} placeholder="com.example.app" /></label>
        <div className="recording-progress"><ListPlus size={17} /><div><b>{recordedSteps.length} 个已录制步骤</b><span>{mode === "record" ? "点击画面后 Reactor 使用最佳语义 Selector 真实执行，并等待下一页面稳定。" : "切换到录制/交互模式后才会操作设备。"}</span></div></div>
        <button className="secondary-button" disabled={recordedSteps.length === 0 || interacting} title="只修改当前 Flow 记录，不会操作或回退设备页面" onClick={() => removeRecordedStep(recordedSteps.length - 1)}><Undo2 size={15} />移除记录最后一步</button>
      </section>

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
                <div
                  ref={mirrorRef}
                  className={`device-mirror ${mode}`}
                  onClick={inspectPoint}
                  onPointerEnter={() => document.body.classList.add("mirror-gesture-lock")}
                  onPointerLeave={() => document.body.classList.remove("mirror-gesture-lock")}
                  title={mode === "record" ? "点击并录制；滚轮/触控板转换为设备滑动" : "点击审查；镜像内滚动不会滚动 Reactor"}
                >
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
            {mode === "record" && (
              <section className="recorded-flow-panel" aria-label="当前录制 Flow">
                <div className="recorded-flow-heading">
                  <div><p className="eyebrow">RECORDED FLOW</p><h3>从本次录制开始的 Step Flow</h3></div>
                  <span>{recordedSteps.length} steps</span>
                </div>
                <div className="flow-editor-tabs" role="tablist">
                  <button className={flowView === "steps" ? "active" : ""} onClick={() => setFlowView("steps")}><ListPlus size={13} />步骤</button>
                  <button className={flowView === "json" ? "active" : ""} onClick={() => setFlowView("json")}><Braces size={13} />完整 Flow JSON</button>
                  <button className={flowView === "yaml" ? "active" : ""} onClick={() => setFlowView("yaml")}><Code2 size={13} />Maestro YAML</button>
                </div>
                <label className="measurement-boundary"><span>测量窗口</span><select value={measurementStart ?? ""} onChange={(event) => { rememberEditorState(); setMeasurementStart(event.target.value === "" ? undefined : Number(event.target.value)); setJsonDirty(false); }}><option value="">尚未指定（全部属于 setup）</option>{recordedSteps.map((_, index) => <option value={index} key={index}>从步骤 {index + 1} 开始 measured</option>)}</select></label>
                {flowView === "steps" && (recordedSteps.length > 0 ? (
                  <ol className="recorded-flow-list">
                    {recordedSteps.map((step, index) => (
                      <li key={`${step.action}-${index}`}>
                        <span>{index + 1}</span>
                        <div><b>{flowStepName(step)} <small>{stepSection(index, measurementStart, teardownStart)}</small></b><code>{flowStepDetail(step)}</code></div>
                        <div className="recorded-step-actions"><button title="逐步回放" disabled={replaying} onClick={() => void replayOneStep(step)}><Play size={12} /></button><button title="上移" onClick={() => moveRecordedStep(index, -1)}><ArrowUp size={12} /></button><button title="下移" onClick={() => moveRecordedStep(index, 1)}><ArrowDown size={12} /></button><button title="删除" onClick={() => removeRecordedStep(index)}><Trash2 size={12} /></button></div>
                      </li>
                    ))}
                  </ol>
                ) : (
                  <div className="recorded-flow-empty"><ListPlus size={20} /><span>尚无步骤。点击、返回或滑动设备镜像后，会按执行顺序持续追加在这里。</span></div>
                ))}
                {flowView === "json" && <div className="flow-source-editor"><textarea value={jsonDraft} spellCheck={false} onChange={(event) => { setJsonDraft(event.target.value); setJsonDirty(true); }} /><div><button className="secondary-button" disabled={!jsonDirty} onClick={() => { setJsonDraft(JSON.stringify(explorerFlow, null, 2)); setJsonDirty(false); setEditorError(""); }}><RotateCcw size={13} />放弃编辑</button><button className="primary-button" disabled={!jsonDirty} onClick={() => void applyJsonDraft()}><Check size={13} />校验并应用</button></div></div>}
                {flowView === "yaml" && <pre className="flow-yaml-preview">{compiledFlow ? maestroPreview(compiledFlow) : "请先指定至少一个 measured 步骤；Rust 校验通过后才会生成实际 Maestro YAML。"}</pre>}
                {promptReferences.length > 0 && <div className="replay-prompts"><b>本次回放输入（不写入 Flow）</b>{promptReferences.map((reference) => <label key={reference}><span>{reference}</span><input type="password" autoComplete="off" value={promptValues[reference] ?? ""} onChange={(event) => setPromptValues((values) => ({ ...values, [reference]: event.target.value }))} /></label>)}</div>}
                {editorError && <div className="flow-editor-error">{editorError}</div>}
                <div className="flow-editor-actions"><button className="secondary-button" onClick={() => void copyFlowSource()}>{copiedStrategy === "flow-source" ? <Check size={13} /> : <Copy size={13} />}{copiedStrategy === "flow-source" ? "已复制" : "复制当前视图"}</button><button className="secondary-button" disabled={!editorUndo} onClick={undoEditorChange}><Undo2 size={13} />撤销编辑</button><button className="primary-button" disabled={!compiledFlow || replaying || activeJobRunning} onClick={() => void replayWholeFlow()}>{replaying ? <RefreshCw size={13} className="spin" /> : <Play size={13} />}{replaying ? "整体回放中" : "整体回放"}</button></div>
              </section>
            )}
            <section className="state-graph-panel" aria-label="AI 状态图探索">
              <div className="state-graph-heading"><div><p className="eyebrow">AI STATE GRAPH · M8.10C</p><h3>只基于真实观察页面建议下一步</h3></div><GitBranch size={17} /></div>
              <p className="state-graph-goal">目标：{goal}</p>
              <div className="state-graph-summary"><span>{graphNodes.length} 个状态</span><span>{graphTransitions.length} 条真实转移</span><span>{providerLabel(ai.provider)}</span></div>
              {graphNodes.length > 0 && <div className="state-graph-nodes">{graphNodes.slice(-5).map((node) => <div className={node.id === currentGraphStateRef.current ? "current" : ""} key={node.id}><span>{node.id.slice(0, 6)}</span><b>{node.label}</b><small>{node.elementCount} elements</small></div>)}</div>}
              {graphTransitions.length > 0 && <div className="state-graph-transitions">{graphTransitions.slice(-4).map((transition) => <span key={transition.id}>{transition.from.slice(0, 4)} → {transition.to.slice(0, 4)} · {transition.action}</span>)}</div>}
              <p className="state-graph-privacy">仅发送脱敏后的可见文本、资源 ID、交互/输入类型和 bounds；不发送截图、输入值或 Secret。</p>
              <button className="secondary-button state-suggest-button" disabled={!snapshot || suggesting || activeJobRunning} onClick={() => void generateNextSuggestion()}>{suggesting ? <RefreshCw size={14} className="spin" /> : <Sparkles size={14} />}{suggesting ? "正在生成安全建议" : "生成下一步建议"}</button>
              {suggestion && <div className={`state-suggestion ${suggestion.dangerous || !suggestion.knownTarget ? "warning" : "safe"}`}><div><b>{flowStepName(suggestion.step)} · {suggestion.label}</b><span>{suggestion.provider} · {suggestion.model}</span></div><p>{suggestion.dangerous ? "命中删除、支付、授权、提交等危险语义；Reactor 拒绝自动执行。" : !suggestion.knownTarget ? "目标未在当前真实 UI 树中命中，Reactor 不会盲目执行；请先在镜像中审查目标。" : suggestion.coordinateFallback ? "这是坐标降级，确认布局无变化后才能执行。" : "目标已在当前页面命中；建议不会自动执行或写入 Flow。"}</p><button className="primary-button" disabled={suggestion.dangerous || !suggestion.knownTarget || replaying} onClick={() => void executeSuggestion()}>{suggestion.coordinateFallback && !suggestionConfirmed ? "审查坐标风险" : "确认、执行并加入 Flow"}</button></div>}
            </section>
            <div className="current-selector-heading"><b>当前 Selector</b><span>{selectedElement ? "待审查 / 待执行" : "尚未选择控件"}</span></div>
            {selectedElement ? (
              <>
                <div className="element-summary">
                  <span className={`element-state ${selectedElement.clickable || selectedElement.editable ? "clickable" : ""}`}>{selectedElement.editable ? selectedElement.password ? "密码输入" : "可输入" : selectedElement.clickable ? "可交互" : "结构元素"}</span>
                  <h3>{elementName(selectedElement)}</h3>
                  <code>{selectedElement.resourceId ?? selectedElement.key}</code>
                  <div><span>位置</span><b>{formatBounds(selectedElement)}</b></div>
                  <div><span>层级</span><b>Depth {selectedElement.depth}</b></div>
                  <div><span>状态</span><b>{selectedElement.enabled ? "Enabled" : "Disabled"}{selectedElement.focused ? " · Focused" : ""}</b></div>
                  <div><span>类型</span><b>{selectedElement.className ?? "未知"}</b></div>
                </div>
                {selectedElement.editable ? (
                  <section className="input-step-editor">
                    <label><span>输入值类型</span><select value={inputKind} onChange={(event) => { setInputKind(event.target.value as typeof inputKind); setSecretStored(false); }}><option value="literal">普通测试文本</option><option value="variableRef">环境变量引用</option><option value="secretRef">系统 Secret 引用</option><option value="promptRef">交互验证码</option><option value="totpRef">TOTP 引用</option></select></label>
                    {inputKind !== "literal" && <label><span>引用名称（写入 Flow）</span><input value={inputReference} onChange={(event) => { setInputReference(event.target.value); setSecretStored(false); }} placeholder={inputKind === "variableRef" ? "TEST_USERNAME" : inputKind === "promptRef" ? "login.sms-code" : inputKind === "totpRef" ? "test-account.totp" : "test-account.password"} /></label>}
                    {(inputKind === "literal" || inputKind === "promptRef") && <label><span>{inputKind === "literal" ? "测试文本" : "仅本次试跑值（不写入 Flow）"}</span><input value={inputValue} onChange={(event) => setInputValue(event.target.value)} type={selectedElement.password ? "password" : "text"} autoComplete="off" /></label>}
                    {(inputKind === "secretRef" || inputKind === "totpRef") && <label><span>{inputKind === "totpRef" ? "Base32 密钥（仅保存到系统凭据库）" : "Secret（仅保存到系统凭据库）"}</span><input value={secretValue} onChange={(event) => { setSecretValue(event.target.value); setSecretStored(false); }} type="password" autoComplete="off" placeholder={secretStored ? "已保存；留空继续使用" : "输入后执行时保存"} /></label>}
                    <label className="input-clear-option"><input type="checkbox" checked={clearBeforeInput} onChange={(event) => setClearBeforeInput(event.target.checked)} /><span>输入前清空原内容</span></label>
                    <p>Flow 只保存引用名称；Secret、TOTP 密钥和交互验证码不会进入 JSON、YAML 预览或日志。</p>
                    <button className="primary-button inspector-execute-button" disabled={inputBusy || interacting || activeJobRunning} onClick={() => { beginRecording(); void recordInput(selectedElement); }}><Play size={15} />{inputBusy ? "正在安全输入" : "在设备上输入并继续"}</button>
                  </section>
                ) : <button className="primary-button inspector-execute-button" disabled={!selectedElement.clickable || interacting || activeJobRunning} title={!appId.trim() ? "执行前需要填写当前 App 包名 / Bundle ID" : "在设备上执行并加入当前 Flow"} onClick={() => { beginRecording(); void recordTap(selectedElement); }}><Play size={15} />在设备上点击并继续</button>}
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
  const interactive = matching.filter((element) => (element.clickable || element.editable) && element.enabled);
  return (interactive.length > 0 ? interactive : matching)
    .sort((left, right) => left.bounds.width * left.bounds.height - right.bounds.width * right.bounds.height)[0];
}

function elementName(element: InspectorElement): string {
  return element.text ?? element.accessibilityText ?? element.resourceId?.split("/").pop() ?? "未命名元素";
}

function inputValueLabel(value: InputValue): string {
  if (typeof value === "string") return "普通测试文本";
  if ("variableRef" in value) return `变量 ${value.variableRef}`;
  if ("secretRef" in value) return `Secret ${value.secretRef}`;
  if ("promptRef" in value) return `验证码 ${value.promptRef}`;
  return `TOTP ${value.totpRef}`;
}

function isDangerousElement(element: InspectorElement): boolean {
  const value = [element.text, element.accessibilityText, element.resourceId].filter(Boolean).join(" ").toLowerCase();
  return ["delete", "remove account", "pay", "purchase", "buy", "checkout", "transfer", "submit", "authorize", "删除", "支付", "购买", "下单", "转账", "授权", "提交", "注销", "退出登录"].some((keyword) => value.includes(keyword));
}

function selectorLabel(selector: InspectorSelectorCandidate["selector"]): string {
  return selector.accessibilityId ?? selector.semanticId ?? selector.text ?? (selector.coordinate ? `${Math.round(selector.coordinate.x)},${Math.round(selector.coordinate.y)}` : "未知 Selector");
}

function flowStepName(step: FlowStep): string {
  if (step.action === "tap") return "点击";
  if (step.action === "swipe") return "滑动";
  if (step.action === "input_text") return "输入文本";
  if (step.action === "pause") return "等待";
  return step.action;
}

function flowStepDetail(step: FlowStep): string {
  if (step.action === "tap") return selectorLabel(step.target);
  if (step.action === "swipe") return `${step.direction.toUpperCase()} · ${step.duration_ms} ms`;
  if (step.action === "input_text") return `${selectorLabel(step.target)} · ${inputValueLabel(step.value)}`;
  if (step.action === "pause") return `${step.duration_ms} ms`;
  return "";
}

function stepSection(index: number, measurementStart: number | undefined, teardownStart: number): "setup" | "measured" | "teardown" {
  if (index >= teardownStart) return "teardown";
  if (measurementStart !== undefined && index >= measurementStart) return "measured";
  return "setup";
}

function collectPromptReferences(flow: Flow): string[] {
  const references = new Set<string>();
  const visit = (steps: FlowStep[]) => {
    for (const step of steps) {
      if (step.action === "input_text" && typeof step.value !== "string" && "promptRef" in step.value) references.add(step.value.promptRef);
      if (step.action === "repeat") visit(step.steps);
    }
  };
  visit(flow.setup);
  visit(flow.measured);
  visit(flow.teardown);
  return [...references].sort();
}

function maestroPreview(compiled: CompiledFlow): string {
  return ["# setup.yaml", compiled.setup.trimEnd(), "", "# measured.yaml", compiled.measured.trimEnd(), "", "# teardown.yaml", compiled.teardown.trimEnd()].join("\n");
}

function cleanError(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason).replace(/^Error:\s*/, "");
}

function graphNode(snapshot: DeviceInspectorSnapshot): ExplorerGraphNode {
  const safeElements = snapshot.elements.filter((element) => !isSystemNoiseElement(element)).map((element) => ({
    id: element.resourceId ?? "",
    text: element.editable || element.password ? "" : safeUiText(element.text ?? element.accessibilityText ?? ""),
    editable: element.editable,
    clickable: element.clickable,
    bounds: element.bounds,
  }));
  const fingerprint = JSON.stringify(safeElements);
  const labels = safeElements.map((element) => element.text).filter(Boolean).slice(0, 3);
  return {
    id: stableShortHash(`${snapshot.platform}:${fingerprint}`),
    label: labels.join(" · ") || `${snapshot.platform} page`,
    elementCount: snapshot.elements.length,
    capturedAt: snapshot.capturedAt,
  };
}

function explorerAiContext(snapshot: DeviceInspectorSnapshot): string {
  return JSON.stringify({
    schemaVersion: 1,
    platform: snapshot.platform,
    viewport: { width: snapshot.viewportWidth, height: snapshot.viewportHeight },
    elements: snapshot.elements.filter((element) => !isSystemNoiseElement(element)).map((element) => ({
      resourceId: element.resourceId,
      text: element.editable || element.password ? "[REDACTED_EDITABLE_VALUE]" : safeUiText(element.text ?? element.accessibilityText ?? ""),
      clickable: element.clickable,
      editable: element.editable,
      password: element.password,
      enabled: element.enabled,
      bounds: element.bounds,
    })),
  });
}

function safeUiText(value: string): string {
  const trimmed = value.trim().slice(0, 160);
  if (/\b[\w.+-]+@[\w.-]+\.[A-Za-z]{2,}\b/.test(trimmed)) return "[REDACTED_EMAIL]";
  if (/\b(?:\d[ -]?){7,}\d\b/.test(trimmed)) return "[REDACTED_NUMBER]";
  return trimmed;
}

function isSystemNoiseElement(element: InspectorElement): boolean {
  const id = element.resourceId?.toLowerCase() ?? "";
  const text = (element.text ?? element.accessibilityText ?? "").trim();
  return id.includes("com.android.systemui") || /^\d{1,2}:\d{2}$/.test(text);
}

function stableShortHash(value: string): string {
  let hash = 0x811c9dc5;
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0).toString(16).padStart(8, "0");
}

function selectorsOverlap(element: InspectorElement, selector: InspectorSelectorCandidate["selector"]): boolean {
  return Boolean(
    selector.semanticId && selector.semanticId === element.resourceId
    || selector.accessibilityId && selector.accessibilityId === element.resourceId
    || selector.text && [element.text, element.accessibilityText].includes(selector.text),
  );
}

function isDangerousSelector(selector: InspectorSelectorCandidate["selector"]): boolean {
  const value = [selector.semanticId, selector.accessibilityId, selector.text].filter(Boolean).join(" ").toLowerCase();
  return ["delete", "remove account", "pay", "purchase", "checkout", "transfer", "submit", "authorize", "删除", "支付", "购买", "下单", "转账", "授权", "提交", "注销"].some((keyword) => value.includes(keyword));
}

function providerLabel(provider: "offline" | "local" | "codex" | "claude" | "cloud"): string {
  return { offline: "Reactor Offline", local: "Local Model", codex: "Codex CLI", claude: "Claude Code", cloud: "Cloud AI" }[provider];
}

function offlineNextSuggestion(snapshot: DeviceInspectorSnapshot, goal: string): ExplorerSuggestion | undefined {
  const keywords = goal.toLowerCase().split(/[^\p{L}\p{N}]+/u).filter((word) => word.length >= 2);
  const candidates = snapshot.elements
    .filter((element) => element.enabled && element.clickable && element.candidates.length > 0 && !isDangerousElement(element))
    .map((element) => {
      const label = elementName(element);
      const lower = label.toLowerCase();
      const relevance = keywords.reduce((score, keyword) => score + (lower.includes(keyword) ? 20 : 0), 0);
      return { element, label, score: relevance + element.candidates[0].score };
    })
    .sort((left, right) => right.score - left.score);
  const best = candidates[0];
  if (!best) return undefined;
  const step: FlowStep = { action: "tap", target: best.element.candidates[0].selector };
  return {
    step,
    label: best.label,
    provider: "reactor-safe-rules",
    model: "deterministic-next-action-v1",
    knownTarget: true,
    dangerous: false,
    coordinateFallback: Boolean(best.element.candidates[0].selector.coordinate),
    executionPoint: {
      x: best.element.bounds.x + best.element.bounds.width / 2,
      y: best.element.bounds.y + best.element.bounds.height / 2,
    },
  };
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
