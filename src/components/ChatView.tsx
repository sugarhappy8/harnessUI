import { useCallback, useDeferredValue, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ArrowDown, Brain, FileText } from "lucide-react";
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
import { EmptyChat, Welcome } from "./Welcome";
import { formatCount } from "../lib/format";
import type { MessageDto, ToolResultDto } from "../lib/types";

/** The reply being written. Text arrives many times a second; when rendering it cannot
 *  keep up (a long answer with code), React shows the latest it has finished instead of
 *  holding up typing and scrolling. */
function LiveReply(props: { text: string }) {
  const text = useDeferredValue(props.text);
  return (
    <div className="chat-text">
      <Markdown text={text} />
    </div>
  );
}

/** How much of the context window the last request filled. */
function ContextMeter(props: { used: number; nCtx: number | null }) {
  if (!props.nCtx) {
    return (
      <span className="shrink-0 text-xs whitespace-nowrap text-fg-muted tabular-nums" title="Context used by the last request">
        {props.used.toLocaleString()} tokens
      </span>
    );
  }
  const pct = Math.min(100, Math.round((props.used / props.nCtx) * 100));
  const tone = pct >= 90 ? "bg-danger" : pct >= 75 ? "bg-warning" : "bg-accent";
  return (
    <span
      className="flex shrink-0 items-center gap-2 text-xs whitespace-nowrap text-fg-muted tabular-nums"
      title={`Context used by the last request: ${props.used.toLocaleString()} of ${props.nCtx.toLocaleString()} tokens`}
    >
      <span className="h-1.5 w-12 overflow-hidden rounded-full bg-bg-muted" aria-hidden>
        <span className={"block h-full rounded-full transition-[width] duration-500 " + tone} style={{ width: `${pct}%` }} />
      </span>
      {formatCount(props.used)} / {formatCount(props.nCtx)} tokens ({pct}%)
    </span>
  );
}

/** Messages rendered at first in a long chat. */
const RECENT_MESSAGES = 80;

export function ChatView() {
  const {
    detail,
    generation,
    turnNotes,
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
  const scrollerRef = useRef<HTMLDivElement>(null);
  // Follow new text only while the reader is at the bottom; scrolling up to read stops it.
  const pinnedRef = useRef(true);
  const [pinned, setPinned] = useState(true);
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

  const onScroll = useCallback(() => {
    const el = scrollerRef.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
    pinnedRef.current = atBottom;
    setPinned(atBottom);
  }, []);
  const toBottom = useCallback((smooth: boolean) => {
    const el = scrollerRef.current;
    if (!el) return;
    el.scrollTo({ top: el.scrollHeight, behavior: smooth ? "smooth" : "auto" });
    pinnedRef.current = true;
    setPinned(true);
  }, []);
  // Opening a chat starts at its end.
  useLayoutEffect(() => {
    toBottom(false);
  }, [chatKey, toBottom]);
  useLayoutEffect(() => {
    if (pinnedRef.current) scrollerRef.current?.scrollTo({ top: scrollerRef.current.scrollHeight });
  }, [scrollKey]);

  const modelRef = detail?.chat.modelRef ?? null;
  let disabledReason: string | null = null;
  // With no chat open, sending starts one with the loaded model.
  if (!detail) disabledReason = runtime?.state === "ready" ? null : "Choose a model to start chatting";
  else if (!modelRef) disabledReason = "Choose a model above to start chatting";
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

  // Stable callbacks, so the memoised messages are not rendered again for every token.
  const editResend = useCallback((m: MessageDto, text: string) => void send(text, m.parentId), [send]);
  const onRegenerate = useCallback((id: number) => void regenerate(id), [regenerate]);
  const onSwitchBranch = useCallback((id: number) => void switchBranch(id), [switchBranch]);

  return (
    <main className="flex min-w-0 flex-1 flex-col">
      <header className="glass @container z-10 flex items-center gap-1.5 border-b border-border px-3 py-2">
        <ModelPicker />
        <RuntimeIndicator />
        <div className="flex-1" />
        {used !== null && <ContextMeter used={used} nCtx={nCtx} />}
        {live?.tokensPerSecond != null && (
          <span className="shrink-0 text-xs whitespace-nowrap text-fg-muted tabular-nums">
            {live.tokensPerSecond.toFixed(1)} tokens/s
          </span>
        )}
        {detail && modelRef?.startsWith("local:") && runtime?.thinking && (
          <button
            type="button"
            aria-pressed={detail.chat.thinking}
            aria-label={`Thinking ${detail.chat.thinking ? "on" : "off"}`}
            title={detail.chat.thinking ? "Thinking is on: slower, often better answers" : "Thinking is off: faster answers"}
            onClick={() =>
              void ipc
                .setChatThinking(detail.chat.id, !detail.chat.thinking)
                .then(() => openChat(detail.chat.id))
                .catch((e) => showError(toErrorInfo(e)))
            }
            className={`flex shrink-0 items-center gap-1.5 rounded-lg px-2 py-1.5 text-sm whitespace-nowrap hover:bg-bg-muted ${detail.chat.thinking ? "text-accent" : "text-fg-muted"}`}
          >
            <Brain size={15} aria-hidden />
            <span className="hidden @3xl:inline">Thinking {detail.chat.thinking ? "on" : "off"}</span>
          </button>
        )}
        {detail && (
          <ToolsPanel
            chatId={detail.chat.id}
            nCtx={nCtx}
            workingFolder={detail.chat.workingFolder ?? null}
            unsupported={!!modelRef?.startsWith("local:") && runtime?.state === "ready" && !runtime.supportsTools}
          />
        )}
        {detail && (
          <button
            type="button"
            onClick={() => setPromptOpen(true)}
            className="flex shrink-0 items-center gap-1.5 rounded-lg px-2 py-1.5 text-sm whitespace-nowrap text-fg-muted hover:bg-bg-muted hover:text-fg"
            title="System prompt"
            aria-label="System prompt"
          >
            <FileText size={15} aria-hidden />
            <span className="hidden @3xl:inline">System prompt</span>
          </button>
        )}
      </header>

      <div className="relative min-h-0 flex-1">
      <div
        ref={scrollerRef}
        onScroll={onScroll}
        className="app-backdrop h-full overflow-y-auto"
        aria-live="polite"
        aria-busy={busy}
      >
        {!detail && <Welcome />}
        {detail && messages.length === 0 && !live && <EmptyChat />}
        <div className={"mx-auto max-w-3xl space-y-6 px-4 py-6" + (detail ? "" : " hidden")}>
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
            <div key={m.id} className="message-row space-y-2">
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
                  onRegenerate={onRegenerate}
                  onEditResend={editResend}
                  onSwitchBranch={onSwitchBranch}
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
            <div className="enter-fade space-y-2" aria-live="polite">
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
                <details className="rounded-lg border border-border bg-bg-subtle px-3 py-2 text-sm" open>
                  <summary className="cursor-pointer text-fg-muted select-none">Thinking…</summary>
                  <div className="mt-1 whitespace-pre-wrap text-fg-muted">{live.reasoning}</div>
                </details>
              )}
              {live.text ? (
                <LiveReply text={live.text} />
              ) : (
                live.tools.every((t) => t.status !== "running") &&
                chatApprovals.length === 0 && (
                  <div className="typing-dots enter-fade" role="status" aria-label="The model is working">
                    <span />
                    <span />
                    <span />
                  </div>
                )
              )}
              {live.notes.map((n, i) => (
                <p key={i} className="text-xs text-fg-muted">
                  {n}
                </p>
              ))}
            </div>
          )}
          {!live && detail && turnNotes?.chatId === detail.chat.id && (
            <div className="mb-6 space-y-1" aria-label="Notes from the last reply">
              {turnNotes.notes.map((n, i) => (
                <p key={i} className="text-xs text-fg-muted">
                  {n}
                </p>
              ))}
            </div>
          )}
          {chatApprovals.map((a) => (
            <ApprovalCard key={a.id} request={a} onDecide={(d, note) => void decideApproval(a.id, d, note)} />
          ))}
        </div>
      </div>
        {!pinned && detail && (
          <button
            type="button"
            onClick={() => toBottom(true)}
            aria-label="Jump to the latest message"
            title="Jump to the latest message"
            className="enter absolute bottom-3 left-1/2 -translate-x-1/2 rounded-full border border-border bg-surface p-2 text-fg-muted shadow-float hover:text-fg"
          >
            <ArrowDown size={16} aria-hidden />
          </button>
        )}
      </div>

      {error && (
        <div className="mx-auto w-full max-w-3xl px-4 pb-2">
          {error.code === "stepCap" ? (
            // Not a failure: the turn stopped at the step limit, and the model can go on.
            <ErrorBanner
              error={error}
              tone="notice"
              onDismiss={() => showError(null)}
              action={
                busy
                  ? undefined
                  : { label: "Continue", onClick: () => void send("Continue where you left off.") }
              }
            />
          ) : (
            <ErrorBanner error={error} onDismiss={() => showError(null)} />
          )}
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
