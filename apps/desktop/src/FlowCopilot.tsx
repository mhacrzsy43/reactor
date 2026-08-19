import { ArrowRight, Bot, Check, RefreshCw, WandSparkles } from "lucide-react";
import { useState } from "react";
import { modifyFlow } from "./api";
import type { FlowModificationProposal } from "./api";
import type { Flow } from "./types";

type ProviderMode = "local" | "codex" | "claude" | "cloud";

interface FlowCopilotProps {
  flow: Flow;
  provider: ProviderMode;
  providerLabel: string;
  endpoint?: string;
  apiKey?: string;
  saveApiKey: boolean;
  useSavedApiKey: boolean;
  model?: string;
  cliExecutable?: string;
  disabled?: boolean;
  locked?: boolean;
  contextHint?: string;
  failureUiTree?: string;
  onCloneDraft?: () => void;
  onApply: (proposal: FlowModificationProposal) => Promise<void>;
}

export function FlowCopilot({
  flow,
  provider,
  providerLabel,
  endpoint,
  apiKey,
  saveApiKey,
  useSavedApiKey,
  model,
  cliExecutable,
  disabled = false,
  locked = false,
  contextHint,
  failureUiTree,
  onCloneDraft,
  onApply,
}: FlowCopilotProps) {
  const [instruction, setInstruction] = useState("");
  const [proposal, setProposal] = useState<FlowModificationProposal>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [messages, setMessages] = useState<Array<{ role: "user" | "assistant"; text: string }>>([]);

  async function propose(requestOverride?: string) {
    const request = requestOverride?.trim() || instruction.trim();
    if (!request) return;
    setBusy(true);
    setError("");
    setProposal(undefined);
    setMessages((current) => [...current, { role: "user", text: request }]);
    try {
      const next = await modifyFlow({
        flow,
        instruction: request,
        failureContext: contextHint,
        uiTree: failureUiTree,
        provider,
        endpoint,
        apiKey,
        saveApiKey,
        useSavedApiKey,
        model,
        cliExecutable,
      });
      if (next.changes.length === 0 && contextHint?.includes("promptRef")) {
        setMessages((current) => [...current, { role: "assistant", text: "这个失败不需要修改 Flow：promptRef 必须保留。请在 Flow 左侧填写本次交互值，然后重新试跑。" }]);
        setInstruction("");
        return;
      }
      if (next.changes.length === 0) throw new Error("AI 没有产生可验证的修复差异；请明确要修改的失败步骤，或使用下方自动修复。");
      setProposal(next);
      setMessages((current) => [...current, { role: "assistant", text: `已生成 ${next.changes.length} 处修改提案；确认前不会改变 Flow。` }]);
    } catch (reason) {
      const message = String(reason);
      setError(message);
      setMessages((current) => [...current, { role: "assistant", text: `提案失败：${message}` }]);
    } finally {
      setBusy(false);
    }
  }

  async function apply() {
    if (!proposal) return;
    setBusy(true);
    setError("");
    try {
      const count = proposal.changes.length;
      await onApply(proposal);
      setMessages((current) => [...current, { role: "assistant", text: `已应用 ${count} 处修改；旧试跑与锁定应失效。` }]);
      setProposal(undefined);
      setInstruction("");
    } catch (reason) {
      setError(`无法应用提案：${String(reason)}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <aside className="flow-copilot" aria-label="Flow Copilot">
      <div className="flow-copilot-heading"><div><Bot size={17} /><span><b>Flow Copilot</b><small>{providerLabel} · 对话修改当前草稿</small></span></div><span className="schema-badge">AI</span></div>
      {locked && <div className="flow-copilot-locked"><b>当前 Flow 已锁定</b><span>为保留试跑证据，不能直接修改历史版本。</span><button className="secondary-button" onClick={onCloneDraft}>复制为新草稿</button></div>}
      {!locked && contextHint && <div className="flow-copilot-context"><b>试跑失败上下文</b><span>{contextHint}</span></div>}
      <div className="flow-copilot-messages">
        {messages.length === 0 && <div className="flow-copilot-empty"><WandSparkles size={18} /><b>直接描述你想怎么改</b><span>例如：把滚动改为 20 次；将等待移到 setup；增加稳定目标页断言。</span></div>}
        {messages.map((message, index) => <div className={`flow-copilot-message ${message.role}`} key={`${message.role}-${index}`}>{message.text}</div>)}
      </div>
      {proposal && <div className="ai-flow-proposal"><div><b>修改提案 · {proposal.changes.length} 处差异</b><span>{proposal.generated.provider} · {proposal.generated.model}</span></div><ol>{proposal.changes.slice(0, 12).map((change) => <li key={change.path}><code>{change.path}</code><span><del>{summarizeChangeValue(change.before)}</del><ArrowRight size={11} /><ins>{summarizeChangeValue(change.after)}</ins></span></li>)}</ol>{proposal.changes.length > 12 && <small>另有 {proposal.changes.length - 12} 处差异，应用后可查看完整 JSON。</small>}<div className="ai-flow-proposal-actions"><button className="secondary-button" disabled={busy} onClick={() => setProposal(undefined)}>放弃</button><button className="primary-button" disabled={busy} onClick={() => void apply()}><Check size={14} />确认并应用</button></div></div>}
      {error && <div className="flow-editor-error">{error}</div>}
      <textarea disabled={locked} maxLength={4000} value={instruction} onChange={(event) => { setInstruction(event.target.value); setProposal(undefined); setError(""); }} placeholder={locked ? "先复制为新草稿，再用自然语言修改" : "告诉 AI 如何修改当前 Flow…"} />
      {contextHint && !locked && <button className="secondary-button flow-copilot-send" disabled={disabled || busy} onClick={() => void propose("只修复当前失败的一个步骤：从失败证据中找到未命中的 Selector；若它不在当前 UI 的精确 Selector 清单中，只把该步骤的 target 替换为清单里语义最接近的原文值。Selector 区分语言和大小写，禁止翻译、删除或重排其他步骤，也不要提前修改尚未观察页面的步骤。")}><WandSparkles size={14} />根据失败证据自动修复</button>}
      <button className="primary-button flow-copilot-send" disabled={locked || disabled || busy || !instruction.trim()} onClick={() => void propose()}>{busy ? <RefreshCw size={14} className="spin" /> : <WandSparkles size={14} />}{busy ? "处理中" : "生成修改提案"}</button>
      <p>AI 只生成提案；appId、平台、Secret 和测量边界受 Rust 规则保护。</p>
    </aside>
  );
}

function summarizeChangeValue(value: unknown): string {
  if (value === undefined) return "∅";
  const serialized = typeof value === "string" ? value : JSON.stringify(value);
  return serialized.length > 90 ? `${serialized.slice(0, 87)}…` : serialized;
}
