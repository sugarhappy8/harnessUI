import { useEffect, useMemo, useRef, useState } from "react";
import { FileText } from "lucide-react";
import { useApp } from "../store/app";
import { MessageItem } from "./MessageItem";
import { Composer } from "./Composer";
import { ModelPicker } from "./ModelPicker";
import { RuntimeIndicator } from "./RuntimeIndicator";
import { SystemPromptDialog } from "./SystemPromptDialog";
import { ErrorBanner } from "./ErrorBanner";
import type { MessageDto } from "../lib/types";

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
  } = useApp();
  const [promptOpen, setPromptOpen] = useState(false);
  const bottomRef = useRef<HTMLDivElement>(null);
  const busy = generation !== null;
  const messages = useMemo(
    () => (detail?.messages ?? []).filter((m) => m.role === "user" || m.role === "assistant"),
    [detail],
  );
  const lastAssistantId = [...messages].reverse().find((m) => m.role === "assistant")?.id;
  const lastText = messages[messages.length - 1]?.text.length ?? 0;

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ block: "end" });
  }, [messages.length, lastText]);

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
    generation?.promptTokens !== null && generation?.promptTokens !== undefined
      ? generation.promptTokens + (generation.completionTokens ?? 0)
      : lastAssistant?.promptTokens !== null && lastAssistant?.promptTokens !== undefined
        ? lastAssistant.promptTokens + (lastAssistant.completionTokens ?? 0)
        : null;
  const nCtx = modelRef?.startsWith("local:") ? (runtime?.nCtx ?? null) : null;

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
        {generation?.tokensPerSecond != null && (
          <span className="text-xs text-fg-muted">{generation.tokensPerSecond.toFixed(1)} tokens/s</span>
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
          {messages.map((m) => (
            <MessageItem
              key={m.id}
              message={m}
              isLastAssistant={m.id === lastAssistantId}
              busy={busy}
              onRegenerate={(id) => void regenerate(id)}
              onEditResend={editResend}
              onSwitchBranch={(id) => void switchBranch(id)}
            />
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
        onSend={(t) => void send(t)}
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
