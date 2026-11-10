import { useEffect, useMemo, useRef, useState } from "react";
import { Brain, FileText } from "lucide-react";
import * as ipc from "../lib/ipc";
import { useApp, toErrorInfo } from "../store/app";
import { MessageItem } from "./MessageItem";
import { Composer } from "./Composer";
import { ModelPicker } from "./ModelPicker";
import { RuntimeIndicator } from "./RuntimeIndicator";
import { SystemPromptDialog } from "./SystemPromptDialog";
import { ErrorBanner } from "./ErrorBanner";
import { ToolCallCard, type ToolCallView } from "./ToolCallCard";
import { ToolsPanel } from "./ToolsPanel";
import { ApprovalCard } from "./ApprovalCard";
import { Markdown } from "./Markdown";
import type { MessageDto, ToolResultDto } from "../lib/types";

/** Messages rendered at first in a long chat. */
const RECENT_MESSAGES = 80;

export function ChatView() {
  const {
    detail,
    generation,
    runtime,
    error,
    send,
    regenerate,
    stop,
    switchBranch,
    setSystemPrompt,
    showError,
    approvals,
    decideApproval,
    openChat,
  } = useApp();
  const [promptOpen, setPromptOpen] = useState(false);
  // Long chats render their latest messages; earlier ones on request.
  const [earlier, setEarlier] = useState<{ chatId: number | null; count: number }>({ chatId: null, count: 0 });
  const bottomRef = useRef<HTMLDivElement>(null);
  const busy = generation !== null;
  const messages = useMemo(
    () => (detail?.messages ?? []).filter((m) => m.role === "user" || m.role === "assistant"),
    [detail],
  );
  // Tool results by call id, shown inside the card of the call that asked for them.
  const results = useMemo(() => {
    const map = new Map<string, ToolResultDto>();
    for (const m of detail?.messages ?? []) if (m.toolResult) map.set(m.toolResult.callId, m.toolResult);
    return map;
  }, [detail]);
  const lastAssistantId = [...messages].reverse().find((m) => m.role === "assistant")?.id;
  const chatKey = detail?.chat.id ?? null;
  const extra = earlier.chatId === chatKey ? earlier.count : 0;
  const hidden = Math.max(0, messages.length - RECENT_MESSAGES - extra);
  const shown = messages.slice(hidden);
  const live = generation && detail && generation.chatId === detail.chat.id ? generation : null;
  const chatApprovals = approvals.filter((a) => detail && a.chatId === detail.chat.id);
  const scrollKey =
    messages.length + (live ? live.text.length + live.tools.length * 1000 + live.reasoning.length : 0) + chatApprovals.length;

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ block: "end" });
  }, [scrollKey]);

  const modelRef = detail?.chat.modelRef ?? null;
  let disabledReason: string | null = null;
  if (!modelRef) disabledReason = "Choose a model above to start chatting";
  else if (modelRef.startsWith("local:") && runtime?.state === "starting")
    disabledReason = "The model is still loading…";
  else if (modelRef.startsWith("local:") && runtime?.state !== "ready")
    disabledReason = "The model is not loaded. Pick it again from the list above.";

  // Context usage: the last known prompt + completion tokens against the context size.
  const lastAssistant = [...messages].reverse().find((m) => m.role === "assistant");
  const used =
    live?.contextUsed != null
      ? live.contextUsed
      : live?.promptTokens != null
        ? live.promptTokens + (live.completionTokens ?? 0)
        : lastAssistant?.promptTokens !== null && lastAssistant?.promptTokens !== undefined
          ? lastAssistant.promptTokens + (lastAssistant.completionTokens ?? 0)
          : null;
  const nCtx = live?.nCtx ?? (modelRef?.startsWith("local:") ? (runtime?.nCtx ?? null) : null);

  function editResend(m: MessageDto, text: string) {
    void send(text, m.parentId);
  }

  return (
    <main className="flex min-w-0 flex-1 flex-col">
      <header className="flex items-center gap-3 border-b border-border px-4 py-2">
        <ModelPicker />
        <RuntimeIndicator />
        <div className="flex-1" />
        {used !== null && (
          <span className="text-xs text-fg-muted" title="Context used by the last request">
            {nCtx
              ? `${used.toLocaleString()} / ${nCtx.toLocaleString()} tokens (${Math.round((used / nCtx) * 100)}%)`
              : `${used.toLocaleString()} tokens`}
          </span>
        )}
        {live?.tokensPerSecond != null && (
          <span className="text-xs text-fg-muted">{live.tokensPerSecond.toFixed(1)} tokens/s</span>
        )}
        {detail && modelRef?.startsWith("local:") && runtime?.thinking && (
          <button
            type="button"
            aria-pressed={detail.chat.thinking}
            title={detail.chat.thinking ? "Thinking is on: slower, often better answers" : "Thinking is off: faster answers"}
            onClick={() =>
              void ipc
                .setChatThinking(detail.chat.id, !detail.chat.thinking)
                .then(() => openChat(detail.chat.id))
                .catch((e) => showError(toErrorInfo(e)))
            }
            className={`flex items-center gap-1 rounded-md px-2 py-1 text-sm hover:bg-bg-muted ${detail.chat.thinking ? "text-fg" : "text-fg-muted"}`}
          >
            <Brain size={14} aria-hidden /> Thinking {detail.chat.thinking ? "on" : "off"}
          </button>
        )}
        {detail && (
          <ToolsPanel
            chatId={detail.chat.id}
            nCtx={nCtx}
            unsupported={!!modelRef?.startsWith("local:") && runtime?.state === "ready" && !runtime.supportsTools}
          />
        )}
        {detail && (
          <button
            type="button"
            onClick={() => setPromptOpen(true)}
            className="flex items-center gap-1 rounded-md px-2 py-1 text-sm text-fg-muted hover:bg-bg-muted hover:text-fg"
            title="System prompt"
          >
            <FileText size={14} aria-hidden /> System prompt
          </button>
        )}
      </header>

      <div className="flex-1 overflow-y-auto" aria-live="polite" aria-busy={busy}>
        <div className="mx-auto max-w-3xl space-y-6 px-4 py-6">
          {messages.length === 0 && (
            <div className="py-16 text-center text-fg-muted">
              {detail ? "Send a message to start." : "Start a new chat or pick one from the list."}
            </div>
          )}
          {hidden > 0 && (
            <div className="text-center">
              <button
                type="button"
                onClick={() => setEarlier({ chatId: chatKey, count: extra + RECENT_MESSAGES })}
                className="rounded-md border border-border px-3 py-1 text-sm text-fg-muted hover:bg-bg-muted"
              >
                Show {Math.min(hidden, RECENT_MESSAGES)} earlier messages ({hidden} hidden)
              </button>
            </div>
          )}
          {shown.map((m) => (
            <div key={m.id} className="space-y-2">
              {(m.text ||
                m.reasoning ||
                m.role === "user" ||
                m.error ||
                m.toolCalls.length === 0 ||
                m.siblingIds.length > 1) && (
                <MessageItem
                  message={m}
                  isLastAssistant={m.id === lastAssistantId}
                  busy={busy}
                  onRegenerate={(id) => void regenerate(id)}
                  onEditResend={editResend}
                  onSwitchBranch={(id) => void switchBranch(id)}
                />
              )}
              {m.toolCalls.map((c) => {
                const r = results.get(c.id);
                const liveTool = live?.tools.find((t) => t.callId === c.id);
                const waiting = chatApprovals.some((a) => a.callId === c.id);
                const stored: ToolCallView = liveTool ?? {
                  callId: c.id,
                  name: c.name,
                  arguments: c.arguments,
                  recovered: c.recovered,
                  status: r?.status ?? (busy ? "running" : "cancelled"),
                  text: r?.text ?? "",
                };
                const view = waiting && stored.status === "running" ? { ...stored, status: "waiting" } : stored;
                return <ToolCallCard key={c.id} call={view} />;
              })}
            </div>
          ))}
          {live && (
            <div className="space-y-2" aria-live="polite">
              {live.tools
                .filter((t) => !messages.some((m) => m.toolCalls.some((c) => c.id === t.callId)))
                .map((t) => (
                  <ToolCallCard
                    key={t.callId}
                    call={
                      t.status === "running" && chatApprovals.some((a) => a.callId === t.callId)
                        ? { ...t, status: "waiting" }
                        : t
                    }
                  />
                ))}
              {live.reasoning && (
                <details className="rounded-md border border-border bg-bg-subtle px-3 py-1.5 text-sm" open>
                  <summary className="cursor-pointer text-fg-muted select-none">Thinking…</summary>
                  <div className="mt-1 whitespace-pre-wrap text-fg-muted">{live.reasoning}</div>
                </details>
              )}
              {live.text ? (
                <div className="leading-relaxed">
                  <Markdown text={live.text} />
                </div>
              ) : (
                live.tools.every((t) => t.status !== "running") &&
                chatApprovals.length === 0 && <div className="text-fg-muted">…</div>
              )}
              {live.notes.map((n, i) => (
                <p key={i} className="text-xs text-fg-muted">
                  {n}
                </p>
              ))}
            </div>
          )}
          {chatApprovals.map((a) => (
            <ApprovalCard key={a.id} request={a} onDecide={(d, note) => void decideApproval(a.id, d, note)} />
          ))}
          <div ref={bottomRef} />
        </div>
      </div>

      {error && (
        <div className="mx-auto w-full max-w-3xl px-4 pb-2">
          <ErrorBanner error={error} onDismiss={() => showError(null)} />
        </div>
      )}
      <Composer
        busy={busy}
        disabledReason={disabledReason}
        onSend={(t, images, documents) => void send(t, undefined, images, documents)}
        onStop={() => void stop()}
      />
      <SystemPromptDialog
        open={promptOpen}
        initial={detail?.chat.systemPrompt ?? ""}
        onOpenChange={setPromptOpen}
        onSave={(t) => void setSystemPrompt(t)}
      />
    </main>
  );
}
