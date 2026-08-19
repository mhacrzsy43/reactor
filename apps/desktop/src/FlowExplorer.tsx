import { AlertTriangle, ArrowDown, ArrowRight, ArrowUp, Braces, Check, Code2, Copy, Crosshair, GitBranch, ListPlus, LockKeyhole, MousePointer2, Pause, Play, RefreshCw, RotateCcw, ScanSearch, ShieldCheck, Sparkles, Smartphone, Trash2, Undo2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { captureDeviceInspector, captureDeviceReplayFrame, compileFlowPreview, confirmFlow, getFlowSecretStatus, performExplorerStep, previewGenerationContext, probeFlow, replayRecordedFlow, saveFlowSecret, trialGeneratedFlow } from "./api";
import type { FlowModificationProposal } from "./api";
import { FlowCopilot } from "./FlowCopilot";
import type { CompiledFlow, Device, DeviceInspectorSnapshot, Flow, FlowLock, FlowStep, GeneratedFlow, InputValue, InspectorElement, InspectorSelectorCandidate, RedactedUiContext, TrialPreparation } from "./types";

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

interface TargetPageCheckpoint {
  stateId: string;
  appId: string;
  elementCount: number;
  capturedAt: string;
  afterStep: number;
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
    provider: "local" | "codex" | "claude" | "cloud";
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
  onPerformanceHandoff: (lock: FlowLock, preparation: TrialPreparation, compiled: CompiledFlow) => void;
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
  onPerformanceHandoff,
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
  const [replayKind, setReplayKind] = useState<"step" | "whole">();
  const [activeReplayStep, setActiveReplayStep] = useState<number>();
  const [promptValues, setPromptValues] = useState<Record<string, string>>({});
  const [graphNodes, setGraphNodes] = useState<ExplorerGraphNode[]>([]);
  const [graphTransitions, setGraphTransitions] = useState<ExplorerGraphTransition[]>([]);
  const [suggesting, setSuggesting] = useState(false);
  const [suggestion, setSuggestion] = useState<ExplorerSuggestion>();
  const [suggestionConfirmed, setSuggestionConfirmed] = useState(false);
  const [sourceContext, setSourceContext] = useState<RedactedUiContext>();
  const [targetAssertion, setTargetAssertion] = useState<FlowStep>();
  const [assertionMode, setAssertionMode] = useState<"visible" | "text" | "enabled">("text");
  const [selectingTargetMarker, setSelectingTargetMarker] = useState(false);
  const [targetCheckpoint, setTargetCheckpoint] = useState<TargetPageCheckpoint>();
  const [gateBusy, setGateBusy] = useState(false);
  const [gatePreparation, setGatePreparation] = useState<TrialPreparation>();
  const [gateLock, setGateLock] = useState<FlowLock>();
  const [gateError, setGateError] = useState("");
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
  const [selectorRefreshAttempt, setSelectorRefreshAttempt] = useState(0);
  const captureInFlight = useRef(false);
  const interactionInFlight = useRef(false);
  const snapshotRef = useRef<DeviceInspectorSnapshot | undefined>(undefined);
  const mirrorRef = useRef<HTMLDivElement>(null);
  const lastWheelGestureAt = useRef(0);
  const teardownStartRef = useRef(0);
  const currentGraphStateRef = useRef<string | undefined>(undefined);
  const editorErrorRef = useRef<HTMLDivElement>(null);
  const replayStepRefs = useRef<Array<HTMLLIElement | null>>([]);

  useEffect(() => {
    if (activeReplayStep !== undefined) {
      replayStepRefs.current[activeReplayStep]?.scrollIntoView({ behavior: "smooth", block: "nearest" });
    }
  }, [activeReplayStep]);

  useEffect(() => {
    snapshotRef.current = snapshot;
  }, [snapshot]);
  const pendingGraphStepRef = useRef<FlowStep | undefined>(undefined);
  const sourceContextRef = useRef<RedactedUiContext | undefined>(undefined);

  useEffect(() => {
    teardownStartRef.current = teardownStart;
  }, [teardownStart]);

  useEffect(() => {
    setGraphNodes([]);
    setGraphTransitions([]);
    setSuggestion(undefined);
    setSourceContext(undefined);
    setTargetAssertion(undefined);
    setSelectingTargetMarker(false);
    setTargetCheckpoint(undefined);
    setGatePreparation(undefined);
    setGateLock(undefined);
    setGateError("");
    currentGraphStateRef.current = undefined;
    pendingGraphStepRef.current = undefined;
    sourceContextRef.current = undefined;
  }, [selectedDeviceId, appId]);

  function observeSnapshot(next: DeviceInspectorSnapshot, step?: FlowStep) {
    if (next.elements.length === 0) return;
    if (!snapshotBelongsToApp(next, appId)) return;
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
      intent: goal.trim() || undefined,
      setup: recordedSteps.slice(0, setupEnd),
      measured: measurementStart === undefined ? [] : recordedSteps.slice(measurementStart, teardownStart),
      teardown: recordedSteps.slice(teardownStart),
    };
  }, [appId, goal, measurementStart, recordedSteps, selectedDevice?.platform, teardownStart]);

  async function applyCopilotProposal(proposal: FlowModificationProposal) {
    const next = proposal.generated.flow;
    const compiled = await compileFlowPreview(next);
    rememberEditorState();
    setRecordedSteps([...next.setup, ...next.measured, ...next.teardown]);
    setMeasurementStart(next.setup.length);
    const nextTeardownStart = next.setup.length + next.measured.length;
    setTeardownStart(nextTeardownStart);
    teardownStartRef.current = nextTeardownStart;
    setCompiledFlow(compiled);
    setJsonDraft(JSON.stringify(next, null, 2));
    setJsonDirty(false);
    setEditorError(`Copilot 已应用 ${proposal.changes.length} 处修改；旧目标页证明、试跑与锁定已失效，请重新回放并确认目标页。`);
    setTargetAssertion(undefined);
    setTargetCheckpoint(undefined);
    setGatePreparation(undefined);
    setGateLock(undefined);
    setSelectingTargetMarker(false);
  }

  const draftReplayFlow = useMemo<Flow>(() => {
    const replay = { ...explorerFlow, intent: undefined };
    if (replay.measured.length > 0 || recordedSteps.length === 0) return replay;
    return {
      ...replay,
      setup: recordedSteps.slice(0, -1),
      measured: recordedSteps.slice(-1),
      teardown: [],
    };
  }, [explorerFlow, recordedSteps]);

  const promptReferences = useMemo(() => collectPromptReferences(explorerFlow), [explorerFlow]);
  const missingReplayPrompt = promptReferences.find((reference) => !promptValues[reference]);
  const replayBlockedReason = activeJobRunning
    ? "性能任务运行期间不能操作设备"
    : jsonDirty
      ? "完整 Flow JSON 尚未应用，请先校验并应用或放弃编辑"
      : recordedSteps.length === 0
        ? "请先录制至少一个步骤"
          : missingReplayPrompt
            ? `请先填写本次回放输入：${missingReplayPrompt}`
            : undefined;
  const lockBlockedReasons = [
    !sourceContext ? "等待保存起始页" : undefined,
    !targetCheckpoint ? "尚未确认当前目标页" : undefined,
    !targetAssertion ? "尚未点选目标页唯一标记" : undefined,
    !compiledFlow ? "尚未指定 measured 步骤或 Flow 校验未通过" : undefined,
    activeJobRunning ? "性能任务正在运行" : undefined,
  ].filter((reason): reason is string => Boolean(reason));

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
        setEditorError(`性能锁定校验未通过：${String(reason)}。你仍可使用“整体回放（草稿）”验证动作。`);
      }
    });
    return () => { cancelled = true; };
  }, [explorerFlow, jsonDirty]);

  useEffect(() => {
    setGatePreparation(undefined);
    setGateLock(undefined);
    setGateError("");
  }, [explorerFlow]);

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
      if (next.elements.length > 0) {
        pendingGraphStepRef.current = undefined;
        setSelectorRefreshAttempt(0);
      }
      if (next.elements.length > 0 && snapshotBelongsToApp(next, appId) && !sourceContextRef.current && appId.trim()) {
        try {
          const context = await previewGenerationContext({
            appId: appId.trim(),
            platform: selectedDevice.platform === "ios" ? "ios" : "android",
            deviceId: selectedDevice.id,
          });
          sourceContextRef.current = context;
          setSourceContext(context);
        } catch (reason) {
          setGateError(`无法保存起始页证明：${cleanError(reason)}`);
        }
      }
      setSelectedElementKey((current) => current && next.elements.some((element) => element.key === current) ? current : undefined);
    } catch (reason) {
      setError(String(reason));
      setLive(false);
    } finally {
      setLoading(false);
      captureInFlight.current = false;
    }
  }, [activeJobRunning, appId, selectedDevice]);

  useEffect(() => {
    if (!snapshot || snapshot.elements.length > 0 || !pendingGraphStepRef.current || activeJobRunning || selectorRefreshAttempt >= 5) return undefined;
    const timer = window.setTimeout(() => {
      void capture().finally(() => setSelectorRefreshAttempt((attempt) => attempt + 1));
    }, 300 + selectorRefreshAttempt * 250);
    return () => window.clearTimeout(timer);
  }, [activeJobRunning, capture, selectorRefreshAttempt, snapshot?.capturedAt, snapshot?.elements.length]);

  useEffect(() => {
    setSnapshot(undefined);
    setSelectedElementKey(undefined);
    setPoint(undefined);
    setError("");
    setLive(false);
    setSelectorRefreshAttempt(0);
    if (selectedDevice && !activeJobRunning) void capture();
  }, [activeJobRunning, capture, selectedDevice?.id, selectedDevice?.platform]);

  useEffect(() => {
    if (!live || activeJobRunning) return undefined;
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") void capture();
    }, 3_000);
    return () => window.clearInterval(timer);
  }, [activeJobRunning, capture, live]);

  useEffect(() => {
    if ((!replaying && !gateBusy) || !selectedDevice || activeJobRunning) return undefined;
    let cancelled = false;
    let inFlight = false;
    const refreshFrame = async () => {
      if (cancelled || inFlight || document.visibilityState !== "visible") return;
      inFlight = true;
      try {
        const frame = await captureDeviceReplayFrame({
          platform: selectedDevice.platform === "ios" ? "ios" : "android",
          deviceId: selectedDevice.id,
        });
        if (cancelled) return;
        setSnapshot((current) => current ? {
          ...current,
          screenshotDataUrl: frame.screenshotDataUrl,
          screenshotWidth: frame.screenshotWidth,
          screenshotHeight: frame.screenshotHeight,
          capturedAt: frame.capturedAt,
        } : current);
      } catch {
        // The final replay result owns error reporting; a missed preview frame is non-fatal.
      } finally {
        inFlight = false;
      }
    };
    void refreshFrame();
    const timer = window.setInterval(() => void refreshFrame(), 650);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [activeJobRunning, gateBusy, replaying, selectedDevice]);

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
    if (replaying || gateBusy) {
      setError("整体回放期间镜像保持只读；Reactor 正在实时刷新设备画面。");
      return;
    }
    const rect = event.currentTarget.getBoundingClientRect();
    const x = ((event.clientX - rect.left) / rect.width) * snapshot.viewportWidth;
    const y = ((event.clientY - rect.top) / rect.height) * snapshot.viewportHeight;
    const hit = hitTest(snapshot.elements, x, y);
    setPoint({ x, y });
    setSelectedElementKey(hit?.key);
    setError("");
    if (selectingTargetMarker) {
      if (!hit) {
        setGateError("这个位置没有可识别元素，请点选目标页独有的稳定文本或语义 ID。");
        return;
      }
      if (!hit.candidates.some((candidate) => isStableSelector(candidate.selector))) {
        setGateError("当前元素只有坐标定位，不能证明目标页；请选择页面标题、完成标记或带语义 ID 的元素。");
        return;
      }
      setSelectingTargetMarker(false);
      addTargetPageAssertion(hit);
      return;
    }
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

  async function beginRecording(restart = false): Promise<boolean> {
    setMode("record");
    setLive(false);
    if (!restart && recordedSteps.length > 0) return true;
    if (!selectedDevice || !appId.trim() || activeJobRunning || interactionInFlight.current) {
      setError(!appId.trim() ? "请先填写当前 App 包名 / Bundle ID，再开始录制。" : "当前无法启动录制，请确认设备可用且没有性能任务运行。");
      return false;
    }
    interactionInFlight.current = true;
    setInteracting(true);
    setInteractingLabel("启动 App 并建立可信录制起点");
    setError("");
    try {
      const initial: FlowStep = { action: "launch_app" };
      const next = await performExplorerStep({
        platform: selectedDevice.platform === "ios" ? "ios" : "android",
        deviceId: selectedDevice.id,
        appId: appId.trim(),
        step: initial,
      });
      if (restart) rememberEditorState();
      setRecordedSteps([initial]);
      teardownStartRef.current = 1;
      setTeardownStart(1);
      setMeasurementStart(undefined);
      setTargetAssertion(undefined);
      setTargetCheckpoint(undefined);
      setSelectingTargetMarker(false);
      setJsonDirty(false);
      setSnapshot(next);
      observeSnapshot(next, initial);
      setSelectedElementKey(undefined);
      setPoint(undefined);
      return true;
    } catch (reason) {
      setMode("inspect");
      setError(`无法建立录制起点，Flow 未写入 launch_app：${cleanError(reason)}`);
      return false;
    } finally {
      setInteracting(false);
      setInteractingLabel("");
      interactionInFlight.current = false;
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
        viewportWidth: snapshotRef.current?.viewportWidth,
        viewportHeight: snapshotRef.current?.viewportHeight,
        runtimeInput,
      });
      setRecordedSteps((current) => {
        const base: FlowStep[] = current;
        const insertAt = Math.min(teardownStartRef.current, base.length);
        const next = [...base.slice(0, insertAt), step, ...base.slice(insertAt)];
        teardownStartRef.current = insertAt + 1;
        setTeardownStart(insertAt + 1);
        return next;
      });
      setSnapshot(next);
      if (next.elements.length > 0) observeSnapshot(next, step);
      else {
        pendingGraphStepRef.current = step;
        setSelectorRefreshAttempt(0);
      }
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
    setRecordedSteps((steps) => {
      const next = steps.filter((_, stepIndex) => stepIndex !== index);
      setTargetAssertion(findDestinationAssertion(next));
      return next;
    });
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
    setTargetAssertion(findDestinationAssertion(editorUndo.steps));
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
      setTargetAssertion(findDestinationAssertion([...parsed.setup, ...parsed.measured]));
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
    if (!selectedDevice || recordedSteps.length === 0) return;
    const missingPrompt = promptReferences.find((reference) => !promptValues[reference]);
    if (missingPrompt) {
      setEditorError(`整体回放前请输入本次验证码：${missingPrompt}`);
      window.setTimeout(() => editorErrorRef.current?.scrollIntoView({ behavior: "smooth", block: "center" }), 0);
      return;
    }
    setReplayKind("whole");
    setActiveReplayStep(0);
    setReplaying(true);
    setLive(false);
    setSelectedElementKey(undefined);
    setPoint(undefined);
    setEditorError("");
    try {
      await compileFlowPreview(draftReplayFlow);
      const next = await replayRecordedFlow({
        platform: draftReplayFlow.platform,
        deviceId: selectedDevice.id,
        flow: draftReplayFlow,
        promptValues,
      }, (completedStepIndex) => {
        setActiveReplayStep(Math.min(completedStepIndex + 1, recordedSteps.length - 1));
      });
      setSnapshot(next);
      observeSnapshot(next);
      setPromptValues({});
      setSelectedElementKey(undefined);
    } catch (reason) {
      setEditorError(`整体回放失败：${cleanError(reason)}`);
      window.setTimeout(() => editorErrorRef.current?.scrollIntoView({ behavior: "smooth", block: "center" }), 0);
    } finally {
      setReplaying(false);
      setReplayKind(undefined);
      setActiveReplayStep(undefined);
    }
  }

  async function replayOneStep(step: FlowStep, stepIndex: number) {
    if (!selectedDevice || activeJobRunning || replaying) return;
    const promptReference = step.action === "input_text" && typeof step.value !== "string" && "promptRef" in step.value ? step.value.promptRef : undefined;
    const runtimeInput = promptReference ? promptValues[promptReference] : undefined;
    if (promptReference && !runtimeInput) {
      setEditorError(`逐步回放前请输入本次验证码：${promptReference}`);
      return;
    }
    const tapTarget = step.action === "tap" ? step.target : undefined;
    let executionPoint = tapTarget?.coordinate;
    if (tapTarget && !executionPoint) {
      if (!snapshot) {
        setEditorError("逐步回放需要先把目标页的镜像加载出来；请点击「刷新画面」，让 Reactor 在当前真实页面解析该步骤的坐标。");
        return;
      }
      if (snapshot.elements.length === 0) {
        setEditorError("当前镜像还没有 Selector 索引（可能正在后台刷新）；请稍候点击「刷新画面」，完成后即可逐步回放。");
        return;
      }
      const hitElement = snapshot.elements.find((element) => selectorsOverlap(element, tapTarget));
      if (!hitElement) {
        setEditorError(`逐步回放需要在当前真实页面命中「${selectorLabel(tapTarget)}」；当前镜像没有该控件，请把设备导航到录制时的页面后刷新画面。`);
        return;
      }
      executionPoint = { x: hitElement.bounds.x + hitElement.bounds.width / 2, y: hitElement.bounds.y + hitElement.bounds.height / 2 };
    }
    setReplayKind("step");
    setActiveReplayStep(stepIndex);
    setReplaying(true);
    setEditorError("");
    try {
      const next = await performExplorerStep({
        platform: explorerFlow.platform,
        deviceId: selectedDevice.id,
        appId: appId.trim(),
        step,
        executionPoint,
        viewportWidth: snapshotRef.current?.viewportWidth,
        viewportHeight: snapshotRef.current?.viewportHeight,
        runtimeInput,
      });
      if (next.elements.length === 0) {
        pendingGraphStepRef.current = step;
        setSelectorRefreshAttempt(0);
      }
      setSnapshot(next);
      observeSnapshot(next, step);
      if (promptReference) setPromptValues((values) => ({ ...values, [promptReference]: "" }));
    } catch (reason) {
      setEditorError(`逐步回放失败：${cleanError(reason)}`);
    } finally {
      setReplaying(false);
      setReplayKind(undefined);
      setActiveReplayStep(undefined);
    }
  }

  async function generateNextSuggestion() {
    if (!snapshot || !appId.trim()) return;
    setSuggesting(true);
    setSuggestion(undefined);
    setSuggestionConfirmed(false);
    setEditorError("");
    try {
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
    if (!await beginRecording()) return;
    await executeRecordedStep(suggestion.step, `AI 建议：${suggestion.label}`, suggestion.executionPoint);
    setSuggestion(undefined);
    setSuggestionConfirmed(false);
  }

  function addTargetPageAssertion(element: InspectorElement) {
    if (targetAssertion) {
      setGateError("当前 Flow 已有目标页断言；如需替换，请先在步骤或 JSON 视图删除原断言。");
      return;
    }
    if (!recordedSteps.some((step) => step.action === "tap" || step.action === "input_text")) {
      setGateError("请先在录制/交互模式完成至少一个进入目标页的操作，再选择目标页标记；否则 Reactor 无法证明它属于导航后的页面。");
      return;
    }
    if (!targetCheckpoint) {
      setGateError("请先把当前镜像页面明确确认为目标页，再点选唯一标记。");
      return;
    }
    const candidate = element.candidates.find((item) => isStableSelector(item.selector));
    if (!candidate) {
      setGateError("该元素只有坐标定位，不能作为目标页唯一性证明；请选择带文本、语义 ID 或 accessibility ID 的元素。");
      return;
    }
    const exactText = element.text ?? element.accessibilityText;
    if (assertionMode === "text" && !exactText) {
      setGateError("该元素没有可验证文本，请改用可见性或启用状态断言。");
      return;
    }
    const target = assertionMode === "text"
      ? { text: exactText }
      : assertionMode === "enabled"
        ? { ...candidate.selector, enabled: element.enabled }
        : candidate.selector;
    const step: FlowStep = { action: "assert_visible", target };
    const insertAt = Math.min(measurementStart ?? targetCheckpoint.afterStep, recordedSteps.length);
    rememberEditorState();
    setRecordedSteps((steps) => [...steps.slice(0, insertAt), step, ...steps.slice(insertAt)]);
    setMeasurementStart(measurementStart === undefined || measurementStart <= insertAt ? insertAt + 1 : measurementStart + 1);
    const nextTeardownStart = teardownStart + 1;
    teardownStartRef.current = nextTeardownStart;
    setTeardownStart(nextTeardownStart);
    setTargetAssertion(step);
    setMode("record");
    setJsonDirty(false);
    setGateError("");
  }

  async function validateLockAndProveTarget() {
    if (!selectedDevice || !compiledFlow || !sourceContext || !targetAssertion) return;
    if (promptReferences.length > 0) {
      setGateError("正式性能任务不能暂停等待 promptRef；请改用本机 Secret、CI Secret、测试验证码服务或预认证状态后再锁定。");
      return;
    }
    setGateBusy(true);
    setGateError("");
    setGatePreparation(undefined);
    setGateLock(undefined);
    try {
      const generated: GeneratedFlow = {
        flow: explorerFlow,
        provider: ai.provider,
        model: ai.model || "provider-default",
        promptTemplateVersion: "interactive-explorer-v1",
        notes: ["Built from real device states, human-confirmed actions, and a selected destination assertion"],
      };
      const preparation = await trialGeneratedFlow(generated, selectedDevice.id, sourceContext);
      setGatePreparation(preparation);
      if (preparation.failure || !preparation.trial) {
        throw new Error(preparation.failure?.message ?? "整体 Maestro 回放未生成可信试跑证据");
      }
      if (preparation.goalEvidence && !preparation.goalEvidence.verified) {
        throw new Error("目标页标记没有通过起始页/目标页唯一性证明");
      }
      const lock = await confirmFlow(preparation);
      setGateLock(lock);
      void capture();
    } catch (reason) {
      setGateError(`无法锁定：${cleanError(reason)}`);
    } finally {
      setGateBusy(false);
    }
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
        <div><p className="eyebrow">INTERACTIVE FLOW EXPLORER · M8.10</p><h1>看见页面，逐步录成 Flow</h1></div>
        <div className="top-actions">
          <span className={`status-pill ${activeJobRunning ? "waiting" : "ready"}`}><span className="status-dot" />{activeJobRunning ? "测试运行中 · 同步已暂停" : gateBusy || replayKind === "whole" ? "Maestro 回放中 · 实时镜像" : replayKind === "step" ? "逐步回放中 · 实时镜像" : interacting ? "正在执行并等待页面稳定" : live ? "低频同步中 · 3 秒" : mode === "record" ? "录制/交互模式" : "审查模式"}</span>
          <button className="secondary-button" disabled={!selectedDevice || loading || interacting || replaying || gateBusy || activeJobRunning} onClick={() => void capture()}>{loading ? <RefreshCw size={16} className="spin" /> : <RefreshCw size={16} />}刷新画面</button>
          <button className="secondary-button" disabled={!selectedDevice || interacting || replaying || gateBusy || activeJobRunning} onClick={() => setLive((value) => !value)}>{live ? <Pause size={16} /> : <Play size={16} />}{live ? "暂停同步" : "开始同步"}</button>
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
        <div className="explorer-toolbar-summary"><Smartphone size={16} /><div><b>{selectedDevice?.id ?? "等待连接"}</b><span>{snapshot ? `${snapshot.screenshotWidth} × ${snapshot.screenshotHeight} PNG · ${snapshot.elements.length} 个 UI 元素 · ${selectedDevice?.platform === "android" ? "ADB 直连镜像" : "simctl 直连镜像"}` : "画面和 UI 树只保存在当前内存中"}</span></div></div>
        <button className="secondary-button" onClick={onRefreshDevices}><RefreshCw size={15} />刷新设备列表</button>
      </section>

      <section className="recording-console card">
        <div className="recording-mode" role="group" aria-label="Flow Explorer 模式">
          <button className={mode === "inspect" ? "active" : ""} onClick={() => { setMode("inspect"); setPendingDanger(undefined); }}>审查模式<span>{selectingTargetMarker ? "正在点选目标页标记" : "只看 Selector"}</span></button>
          <button className={mode === "record" ? "active" : ""} onClick={() => { setSelectingTargetMarker(false); void beginRecording(); }}>录制/交互模式<span>真实启动 App 后开始录制</span></button>
        </div>
        <label className="recording-app-id"><span>当前 App 包名 / Bundle ID</span><input value={appId} onChange={(event) => onAppIdChange(event.target.value)} placeholder="com.example.app" /></label>
        <div className="recording-progress"><ListPlus size={17} /><div><b>{recordedSteps.length} 个已录制步骤</b><span>{mode === "record" ? "点击画面后 Reactor 使用最佳语义 Selector 真实执行，并等待下一页面稳定。" : "切换到录制/交互模式后才会操作设备。"}</span></div></div>
        <button className="secondary-button" disabled={recordedSteps.length === 0 || interacting || replaying || activeJobRunning} title="真实重启 App，并用新的 launch_app 替换当前草稿；可撤销" onClick={() => void beginRecording(true)}><RotateCcw size={15} />从可信起点重新录制</button>
        <button className="secondary-button" disabled={recordedSteps.length === 0 || interacting} title="只修改当前 Flow 记录，不会操作或回退设备页面" onClick={() => removeRecordedStep(recordedSteps.length - 1)}><Undo2 size={15} />移除记录最后一步</button>
      </section>

      {pendingDanger && <div className="explorer-guard danger"><AlertTriangle size={18} /><div><b>检测到潜在敏感操作：{elementName(pendingDanger)}</b><span>这可能触发删除、支付、授权、提交或退出登录。只有你明确再次确认后，Reactor 才会在当前测试目标执行并写入 Flow。</span></div><button className="secondary-button" onClick={() => setPendingDanger(undefined)}>取消</button><button className="danger-confirm-button" disabled={interacting || activeJobRunning} onClick={() => void recordTap(pendingDanger)}>确认风险并执行</button></div>}

      {activeJobRunning && <div className="explorer-guard"><ShieldCheck size={17} /><div><b>性能测量隔离已生效</b><span>Reactor 不会在任何运行任务期间截屏或读取 UI 树。任务结束后可继续探索。</span></div></div>}
      {error && <div className="error-banner explorer-error">{error}</div>}

      {devices.length === 0 ? (
        <section className="card explorer-empty"><Smartphone size={32} /><h2>启动一个模拟器后开始探索</h2><p>支持 Android Emulator、Android 真机和 iOS Simulator；点击“刷新设备列表”后无需另外安装 Maestro。</p><button className="primary-button" onClick={onRefreshDevices}><RefreshCw size={16} />刷新设备列表</button></section>
      ) : (
        <div className="explorer-grid">
          <section className="card explorer-device-card">
            <div className="card-heading"><div className="heading-icon purple"><MousePointer2 size={18} /></div><div><h2>设备画面</h2><p>{replaying || gateBusy ? `${replayKind === "step" ? "当前步骤" : "Maestro"}正在真实执行；镜像约每 650 ms 刷新一次。` : mode === "record" ? "点击控件会真实执行、追加步骤并刷新下一页面。" : "点击只审查控件，不会改变 App。"}</p></div>{snapshot && <span className="schema-badge">{new Date(snapshot.capturedAt).toLocaleTimeString()}</span>}</div>
            <div className="device-mirror-stage">
              {snapshot ? (
                <div
                  ref={mirrorRef}
                  className={`device-mirror ${mode}${selectingTargetMarker ? " selecting-target" : ""}`}
                  onClick={inspectPoint}
                  onPointerEnter={() => document.body.classList.add("mirror-gesture-lock")}
                  onPointerLeave={() => document.body.classList.remove("mirror-gesture-lock")}
                  title={selectingTargetMarker ? "点选目标页独有的稳定文本或语义 ID；不会操作设备" : mode === "record" ? "点击并录制；滚轮/触控板转换为设备滑动" : "点击审查；镜像内滚动不会滚动 Reactor"}
                >
                  <img src={snapshot.screenshotDataUrl} alt={`${selectedDevice?.name ?? selectedDevice?.id} 当前画面`} draggable={false} />
                  {selectedElement && <span className="element-highlight" style={highlightStyle(selectedElement, snapshot)}><span>{elementName(selectedElement)}</span></span>}
                  {point && <span className="inspection-point" style={{ left: `${(point.x / snapshot.viewportWidth) * 100}%`, top: `${(point.y / snapshot.viewportHeight) * 100}%` }} />}
                  {(replaying || gateBusy) && <span className="mirror-replay-indicator"><RefreshCw size={12} className="spin" />{replayKind === "step" ? "步骤实时回放" : "Maestro 实时回放"}</span>}
                  {selectingTargetMarker && <span className="mirror-target-indicator"><Crosshair size={12} />点选目标页标记 · 不会操作设备</span>}
                  {interacting && <span className="mirror-interaction-overlay"><RefreshCw size={22} className="spin" /><b>{interactingLabel}</b><small>正在操作设备并等待下一页面稳定</small></span>}
                </div>
              ) : (
                <div className="mirror-placeholder">{loading || interacting ? <RefreshCw size={28} className="spin" /> : <ScanSearch size={32} />}<b>{interacting ? "正在执行步骤并等待下一页面" : loading ? "正在同步画面与 UI 树" : "等待首次画面"}</b><span>截图与 UI 树并行获取，不写入测试产物。</span></div>
              )}
            </div>
            {snapshot?.warnings.map((warning) => <div className="explorer-warning" key={warning}>{warning}</div>)}
            {snapshot && snapshot.elements.length === 0 && pendingGraphStepRef.current && (
              <div className="explorer-warning">
                {selectorRefreshAttempt < 5 ? `正在恢复 Selector 索引（${selectorRefreshAttempt + 1}/5），完成后即可点选目标页标记。` : "Selector 索引自动恢复未成功；请确认页面已稳定后手动刷新。"}
                <button className="secondary-button" disabled={loading || captureInFlight.current} onClick={() => { setSelectorRefreshAttempt(0); void capture(); }}><RefreshCw size={13} className={loading ? "spin" : ""} />立即刷新 Selector</button>
              </div>
            )}
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
                <label className="measurement-boundary"><span>测量窗口</span><select value={measurementStart ?? ""} onChange={(event) => { rememberEditorState(); setMeasurementStart(event.target.value === "" ? undefined : Number(event.target.value)); setJsonDirty(false); }}><option value="">尚未指定（全部属于 setup）</option>{recordedSteps.map((_, index) => <option value={index} key={index}>从步骤 {index + 1} 开始 measured</option>)}{measurementStart === recordedSteps.length && <option value={recordedSteps.length}>从下一条性能操作开始 measured（已自动建议）</option>}</select></label>
                {flowView === "steps" && (recordedSteps.length > 0 ? (
                  <ol className="recorded-flow-list">
                    {recordedSteps.map((step, index) => (
                      <li ref={(node) => { replayStepRefs.current[index] = node; }} className={activeReplayStep === index ? "replay-active" : undefined} aria-current={activeReplayStep === index ? "step" : undefined} key={`${step.action}-${index}`}>
                        <span>{index + 1}</span>
                        <div><b>{flowStepName(step)} <small>{stepSection(index, measurementStart, teardownStart)}</small></b><code>{flowStepDetail(step)}</code></div>
                        <div className="recorded-step-actions"><button title={step.action === "reset_app_state" ? "清除应用数据属于破坏性操作，只能通过整体回放执行" : "逐步回放"} disabled={replaying || step.action === "reset_app_state"} onClick={() => void replayOneStep(step, index)}><Play size={12} /></button><button title="上移" onClick={() => moveRecordedStep(index, -1)}><ArrowUp size={12} /></button><button title="下移" onClick={() => moveRecordedStep(index, 1)}><ArrowDown size={12} /></button><button title="删除" onClick={() => removeRecordedStep(index)}><Trash2 size={12} /></button></div>
                      </li>
                    ))}
                  </ol>
                ) : (
                  <div className="recorded-flow-empty"><ListPlus size={20} /><span>尚无步骤。点击、返回或滑动设备镜像后，会按执行顺序持续追加在这里。</span></div>
                ))}
                {flowView === "json" && <div className="flow-source-editor"><textarea value={jsonDraft} spellCheck={false} onChange={(event) => { setJsonDraft(event.target.value); setJsonDirty(true); }} /><div><button className="secondary-button" disabled={!jsonDirty} onClick={() => { setJsonDraft(JSON.stringify(explorerFlow, null, 2)); setJsonDirty(false); setEditorError(""); }}><RotateCcw size={13} />放弃编辑</button><button className="primary-button" disabled={!jsonDirty} onClick={() => void applyJsonDraft()}><Check size={13} />校验并应用</button></div></div>}
                {flowView === "yaml" && <pre className="flow-yaml-preview">{compiledFlow ? maestroPreview(compiledFlow) : "请先指定至少一个 measured 步骤；Rust 校验通过后才会生成实际 Maestro YAML。"}</pre>}
                {promptReferences.length > 0 && <div className="replay-prompts"><b>本次回放输入（不写入 Flow）</b>{promptReferences.map((reference) => <label key={reference}><span>{reference}</span><input type="password" autoComplete="off" value={promptValues[reference] ?? ""} onChange={(event) => setPromptValues((values) => ({ ...values, [reference]: event.target.value }))} /></label>)}</div>}
                {editorError && <div ref={editorErrorRef} className="flow-editor-error">{editorError}</div>}
                {!editorError && replayBlockedReason && <div className="flow-editor-error">整体回放暂不可用：{replayBlockedReason}</div>}
                <div className="flow-editor-actions"><button className="secondary-button" onClick={() => void copyFlowSource()}>{copiedStrategy === "flow-source" ? <Check size={13} /> : <Copy size={13} />}{copiedStrategy === "flow-source" ? "已复制" : "复制当前视图"}</button><button className="secondary-button" disabled={!editorUndo} onClick={undoEditorChange}><Undo2 size={13} />撤销编辑</button><button className="primary-button" disabled={Boolean(replayBlockedReason) || replaying} title={replayBlockedReason} onClick={() => void replayWholeFlow()}>{replaying ? <RefreshCw size={13} className="spin" /> : <Play size={13} />}{replaying ? replayKind === "step" ? "单步回放中" : "整体回放中" : "整体回放（草稿）"}</button></div>
                {compiledFlow && <FlowCopilot
                  flow={explorerFlow}
                  provider={ai.provider}
                  providerLabel={providerLabel(ai.provider)}
                  endpoint={ai.endpoint}
                  apiKey={ai.apiKey}
                  saveApiKey={ai.saveApiKey}
                  useSavedApiKey={ai.useSavedApiKey}
                  model={ai.model || undefined}
                  cliExecutable={ai.cliExecutable}
                  disabled={activeJobRunning || replaying || gateBusy}
                  locked={Boolean(gateLock)}
                  onCloneDraft={() => { setGateLock(undefined); setGatePreparation(undefined); setGateError("已从锁定 Flow 复制为可编辑草稿；修改后需重新回放和证明目标页。"); }}
                  onApply={applyCopilotProposal}
                />}
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
            <section className="assertion-builder-panel" aria-label="目标页断言与性能测试衔接">
              <div className="state-graph-heading"><div><p className="eyebrow">ASSERTION BUILDER · M8.10D</p><h3>证明目标页，再锁定性能 Flow</h3></div><ShieldCheck size={17} /></div>
              <div className="assertion-readiness">
                <span className={sourceContext ? "ready" : "waiting"}>{sourceContext ? `起始页已保存 · ${sourceContext.preview.elementCount} elements` : "等待保存起始页"}</span>
                <span className={targetAssertion ? "ready" : "waiting"}>{targetAssertion ? `目标断言 · ${flowStepDetail(targetAssertion)}` : targetCheckpoint ? "目标页已确认 · 等待点选唯一标记" : "尚未确认目标页"}</span>
                <span className={compiledFlow ? "ready" : "waiting"}>{compiledFlow ? `${explorerFlow.measured.length} 个 measured 步骤 · Rust 校验通过` : "请指定至少一个 measured 步骤"}</span>
              </div>
              {targetCheckpoint && <div className="target-page-checkpoint"><div><ShieldCheck size={15} /><span><b>当前目标页已确认 · {targetCheckpoint.stateId.slice(0, 6)}</b><small>{targetCheckpoint.appId} · {targetCheckpoint.elementCount} elements · Flow 第 {targetCheckpoint.afterStep} 步后</small></span></div><button disabled={Boolean(targetAssertion) || gateBusy} onClick={() => { setTargetCheckpoint(undefined); setSelectingTargetMarker(false); setSelectedElementKey(undefined); setGateError("请把设备停在新的目标页，再重新确认当前页。"); }}>更换目标页</button></div>}
              <p className="state-graph-privacy">请在目标页点选一个只属于该页面的稳定文本或语义 ID。坐标不能证明目标页，整体回放成功也不能替代唯一性证明。</p>
              <label className="assertion-mode"><span>断言类型</span><select value={assertionMode} disabled={Boolean(targetAssertion)} onChange={(event) => setAssertionMode(event.target.value as typeof assertionMode)}><option value="visible">元素可见</option><option value="text">文本完全匹配</option><option value="enabled">启用状态与当前一致</option></select></label>
              <button
                className="secondary-button state-suggest-button"
                disabled={Boolean(targetAssertion) || gateBusy || replaying || activeJobRunning}
                onClick={() => {
                  if (!selectingTargetMarker) {
                    if (!recordedSteps.some((step) => step.action === "tap" || step.action === "input_text")) {
                      setGateError("请先录制至少一个进入目标页的点击或输入操作，再把当前镜像页面确认为目标页。");
                      return;
                    }
                    if (!snapshot || !snapshotBelongsToApp(snapshot, appId)) {
                      setGateError("当前镜像属于 Android 系统桌面或其他 App，不能确认为目标 App 页面。请先回到目标 App。");
                      return;
                    }
                    const checkpointNode = graphNode(snapshot);
                    setTargetCheckpoint({
                      stateId: checkpointNode.id,
                      appId: appId.trim(),
                      elementCount: snapshot.elements.length,
                      capturedAt: snapshot.capturedAt,
                      afterStep: recordedSteps.length,
                    });
                    setSelectedElementKey(undefined);
                    setSelectingTargetMarker(true);
                    setMode("inspect");
                    setGateError("已将当前镜像视为你明确选择的目标页。请点选该页独有的稳定文本或语义 ID；本次点击只选择，不会操作设备。");
                    mirrorRef.current?.scrollIntoView({ behavior: "smooth", block: "center" });
                    return;
                  }
                  setSelectingTargetMarker(false);
                  setGateError("已退出目标标记点选模式。");
                }}
              ><Crosshair size={14} />{targetAssertion ? "目标页断言已加入 Flow" : selectingTargetMarker ? "取消点选目标页标记" : targetCheckpoint ? "继续点选目标页标记" : "将当前页设为目标页并点选标记"}</button>
              {gatePreparation?.goalEvidence && <div className={`goal-proof ${gatePreparation.goalEvidence.verified ? "verified" : "failed"}`}><b>{gatePreparation.goalEvidence.verified ? "目标页唯一性证明通过" : "目标页唯一性证明失败"}</b><span>“{gatePreparation.goalEvidence.marker}” · 起始页 {gatePreparation.goalEvidence.sourceContainsMarker ? "存在" : "不存在"} · 目标页 {gatePreparation.goalEvidence.destinationContainsMarker ? "存在" : "不存在"}</span><small>{gatePreparation.goalEvidence.sourceElements} → {gatePreparation.goalEvidence.destinationElements} elements</small></div>}
              {gateError && <div className="flow-editor-error">{gateError}</div>}
              {!gateLock && lockBlockedReasons.length > 0 && <div className="lock-blocked-reasons"><b>完成以下条件后可回放并锁定</b>{lockBlockedReasons.map((reason) => <span key={reason}>○ {reason}</span>)}</div>}
              {gateLock ? <><div className="explorer-lock"><LockKeyhole size={15} /><div><b>Flow 已锁定</b><code>{gateLock.flowHash}</code></div></div><button className="primary-button state-suggest-button" onClick={() => compiledFlow && onPerformanceHandoff(gateLock, gatePreparation!, compiledFlow)}><ArrowRight size={14} />交给正式性能测试</button></> : <button className="primary-button state-suggest-button" disabled={gateBusy || !compiledFlow || !sourceContext || !targetAssertion || activeJobRunning} onClick={() => void validateLockAndProveTarget()}>{gateBusy ? <RefreshCw size={14} className="spin" /> : <LockKeyhole size={14} />}{gateBusy ? "整体 Maestro 回放与证明中" : "整体回放、证明并锁定"}</button>}
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
                    <button className="primary-button inspector-execute-button" disabled={inputBusy || interacting || activeJobRunning} onClick={() => { void beginRecording().then((ready) => { if (ready) return recordInput(selectedElement); }); }}><Play size={15} />{inputBusy ? "正在安全输入" : "在设备上输入并继续"}</button>
                  </section>
                ) : <button className="primary-button inspector-execute-button" disabled={!selectedElement.clickable || interacting || activeJobRunning} title={!appId.trim() ? "执行前需要填写当前 App 包名 / Bundle ID" : "在设备上执行并加入当前 Flow"} onClick={() => { void beginRecording().then((ready) => { if (ready) return recordTap(selectedElement); }); }}><Play size={15} />在设备上点击并继续</button>}
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
  return ["delete", "remove account", "pay", "purchase", "buy", "checkout", "transfer", "submit", "authorize", "permissioncontroller", "allow permission", "删除", "支付", "购买", "下单", "转账", "授权", "允许访问", "提交", "注销", "退出登录"].some((keyword) => value.includes(keyword));
}

function selectorLabel(selector: InspectorSelectorCandidate["selector"]): string {
  const identity = selector.accessibilityId ?? selector.semanticId ?? selector.text ?? (selector.coordinate ? `${Math.round(selector.coordinate.x)},${Math.round(selector.coordinate.y)}` : "未知 Selector");
  return selector.enabled === undefined ? identity : `${identity} · enabled=${selector.enabled}`;
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
  if (step.action === "wait_for" || step.action === "assert_visible") return selectorLabel(step.target);
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

function snapshotBelongsToApp(snapshot: DeviceInspectorSnapshot, appId: string): boolean {
  if (snapshot.platform !== "android" || !appId.trim()) return true;
  const packages = new Set(snapshot.elements.map((element) => element.packageName).filter(Boolean));
  return packages.size === 0 || packages.has(appId.trim());
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

function isStableSelector(selector: InspectorSelectorCandidate["selector"]): boolean {
  return Boolean(selector.semanticId || selector.accessibilityId || selector.text);
}

function findDestinationAssertion(steps: FlowStep[]): FlowStep | undefined {
  return [...steps].reverse().find((step) => step.action === "assert_visible" && isStableSelector(step.target));
}

function isDangerousSelector(selector: InspectorSelectorCandidate["selector"]): boolean {
  const value = [selector.semanticId, selector.accessibilityId, selector.text].filter(Boolean).join(" ").toLowerCase();
  return ["delete", "remove account", "pay", "purchase", "checkout", "transfer", "submit", "authorize", "permissioncontroller", "allow permission", "删除", "支付", "购买", "下单", "转账", "授权", "允许访问", "提交", "注销"].some((keyword) => value.includes(keyword));
}

function providerLabel(provider: "local" | "codex" | "claude" | "cloud"): string {
  return { local: "Local Model", codex: "Codex CLI", claude: "Claude Code", cloud: "Cloud AI" }[provider];
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
