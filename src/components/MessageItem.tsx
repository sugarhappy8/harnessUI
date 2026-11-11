import { memo, useEffect, useState } from "react";
import { ChevronLeft, ChevronRight, Copy, FileText, Pencil, RotateCcw, Check } from "lucide-react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import type { MessageDto } from "../lib/types";
import { Markdown } from "./Markdown";
import { ErrorBanner } from "./ErrorBanner";
import { getAttachment, isTauri } from "../lib/ipc";

/** An attached image, loaded from the backend as a data: URI. */
function AttachedImage(props: { id: number }) {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    getAttachment(props.id)
      .then((s) => alive && setSrc(s))
      .catch(() => alive && setSrc(null));
    return () => {
      alive = false;
    };
  }, [props.id]);
  if (!src) return <div className="h-24 w-24 rounded-md bg-bg-muted" aria-label="Image" />;
  return <img src={src} alt="Attached image" className="max-h-48 max-w-xs rounded-md border border-border object-contain" />;
}

interface Props {
  message: MessageDto;
  isLastAssistant: boolean;
  busy: boolean;
  onRegenerate: (id: number) => void;
  onEditResend: (message: MessageDto, newText: string) => void;
  onSwitchBranch: (messageId: number) => void;
}

function BranchSwitcher(props: { message: MessageDto; onSwitch: (id: number) => void; busy: boolean }) {
  const ids = props.message.siblingIds;
  if (ids.length < 2) return null;
  const index = ids.indexOf(props.message.id);
  const prev = index > 0 ? ids[index - 1] : undefined;
  const next = index < ids.length - 1 ? ids[index + 1] : undefined;
  return (
    <span className="inline-flex items-center gap-1 text-xs text-fg-muted">
      <button
        type="button"
        aria-label="Previous version"
        disabled={prev === undefined || props.busy}
        onClick={() => prev !== undefined && props.onSwitch(prev)}
        className="disabled:opacity-30"
      >
        <ChevronLeft size={14} aria-hidden />
      </button>
      {index + 1} / {ids.length}
      <button
        type="button"
        aria-label="Next version"
        disabled={next === undefined || props.busy}
        onClick={() => next !== undefined && props.onSwitch(next)}
        className="disabled:opacity-30"
      >
        <ChevronRight size={14} aria-hidden />
      </button>
    </span>
  );
}

/** Memoised: while a reply streams in, the messages above it do not render again. */
export const MessageItem = memo(function MessageItem(props: Props) {
  const m = props.message;
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(m.text);
  const [copied, setCopied] = useState(false);

  async function copy() {
    try {
      if (isTauri()) await writeText(m.text);
      else await navigator.clipboard.writeText(m.text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    } catch {
      setCopied(false);
    }
  }

  if (m.role === "user") {
    return (
      <div className="group enter flex flex-col items-end">
        {editing ? (
          <form
            className="w-full max-w-[85%]"
            onSubmit={(e) => {
              e.preventDefault();
              const text = draft.trim();
              if (text) props.onEditResend(m, text);
              setEditing(false);
            }}
          >
            <textarea
              aria-label="Edit message"
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              rows={Math.min(10, draft.split("\n").length + 1)}
              className="w-full rounded-lg border border-accent bg-bg p-3"
            />
            <div className="mt-1 flex justify-end gap-2 text-sm">
              <button type="button" onClick={() => setEditing(false)} className="px-2 py-1">
                Cancel
              </button>
              <button type="submit" className="rounded-md bg-accent px-3 py-1 text-accent-fg">
                Send
              </button>
            </div>
          </form>
        ) : (
          <div className="flex max-w-[85%] flex-col items-end gap-2">
            {(m.imageIds ?? []).length > 0 && (
              <div className="flex flex-wrap justify-end gap-2">
                {m.imageIds.map((id) => (
                  <AttachedImage key={id} id={id} />
                ))}
              </div>
            )}
            {(m.documents ?? []).length > 0 && (
              <div className="flex flex-wrap justify-end gap-1">
                {m.documents.map((d) => (
                  <span key={d.id} className="flex items-center gap-1 rounded-md border border-border px-2 py-1 text-xs">
                    <FileText size={12} aria-hidden /> {d.name}
                  </span>
                ))}
              </div>
            )}
            {m.text && (
              <div className="chat-text rounded-2xl rounded-br-md border border-accent/20 bg-accent/10 px-3.5 py-2 whitespace-pre-wrap">
                {m.text}
              </div>
            )}
          </div>
        )}
        <div className="mt-1 flex items-center gap-2 opacity-70 group-hover:opacity-100">
          <BranchSwitcher message={m} onSwitch={props.onSwitchBranch} busy={props.busy} />
          {!editing && (
            <button
              type="button"
              aria-label="Edit and resend"
              title="Edit and resend"
              disabled={props.busy}
              onClick={() => {
                setDraft(m.text);
                setEditing(true);
              }}
              className="text-fg-muted hover:text-fg disabled:opacity-30"
            >
              <Pencil size={13} aria-hidden />
            </button>
          )}
        </div>
      </div>
    );
  }

  if (m.role !== "assistant") return null;

  return (
    <div className="group enter">
      {m.reasoning && (
        <details className="mb-2 rounded-lg border border-border bg-bg-subtle px-3 py-2 text-sm">
          <summary className="cursor-pointer text-fg-muted select-none">Thinking</summary>
          <div className="mt-1 whitespace-pre-wrap text-fg-muted">{m.reasoning}</div>
        </details>
      )}
      <div className="chat-text">
        {m.text ? (
          <Markdown text={m.text} />
        ) : m.status === "streaming" ? (
          <span className="text-fg-muted" aria-live="polite">
            …
          </span>
        ) : null}
      </div>
      {m.error && (
        <div className="mt-2">
          <ErrorBanner error={m.error} />
        </div>
      )}
      <div className="mt-1 flex items-center gap-3 text-xs text-fg-muted opacity-70 group-hover:opacity-100">
        <BranchSwitcher message={m} onSwitch={props.onSwitchBranch} busy={props.busy} />
        {m.status === "stopped" && <span>Stopped</span>}
        {m.tokensPerSecond !== null && m.status !== "streaming" && (
          <span title="Generation speed">{m.tokensPerSecond.toFixed(1)} tokens/s</span>
        )}
        {m.text && (
          <button type="button" aria-label="Copy message" onClick={() => void copy()}>
            {copied ? <Check size={13} aria-hidden /> : <Copy size={13} aria-hidden />}
          </button>
        )}
        {props.isLastAssistant && !props.busy && (
          <button
            type="button"
            aria-label="Regenerate"
            title="Regenerate"
            onClick={() => props.onRegenerate(m.id)}
          >
            <RotateCcw size={13} aria-hidden />
          </button>
        )}
      </div>
    </div>
  );
});
