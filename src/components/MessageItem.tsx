import { memo, useEffect, useState } from "react";
import { AtSign, ChevronLeft, ChevronRight, Copy, FileText, Pencil, RotateCcw, Check, Sparkles, X } from "lucide-react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import type { ImageInput, MessageDto } from "../lib/types";
import { STUDIO_CHAT_LINK } from "../lib/words";
import { toErrorInfo } from "../store/app";
import { useStudio } from "../store/studio";
import { Markdown } from "./Markdown";
import { useCitations } from "./sources/useCitations";
import { ErrorBanner } from "./ErrorBanner";
import { getAttachment, isTauri } from "../lib/ipc";
import { PinMessageButton } from "./context/PinMessageButton";

/** A `data:` URI's picture as the backend takes one, named `fileName`; `null` for a URI that
 *  is not base64. */
function pictureOf(uri: string, fileName: string): ImageInput | null {
  const [, mime, data] = /^data:([^;,]+);base64,(.+)$/.exec(uri) ?? [];
  return mime && data ? { mime, fileName, data } : null;
}

/** [0.1.0 B11] *Open in Studio* on an attached picture: its button's words, and what it does
 *  with the picture's `data:` URI. */
interface OpenInStudio {
  label: string;
  busy: boolean;
  open(src: string): void;
}

/** An attached image, loaded from the backend as a data: URI. `small`: a thumbnail. With
 *  `studio`, it has *Open in Studio* once loaded. */
function AttachedImage(props: { id: number; small?: boolean; studio?: OpenInStudio }) {
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
  if (!src) return <div className={(props.small ? "h-16 w-16" : "h-24 w-24") + " rounded-md bg-bg-muted"} aria-label="Image" />;
  if (props.small) {
    return <img src={src} alt="Attached image" className="h-16 w-16 rounded-md border border-border object-cover" />;
  }
  const picture = (
    <img src={src} alt="Attached image" className="max-h-48 max-w-xs rounded-md border border-border object-contain" />
  );
  const studio = props.studio;
  if (!studio) return picture;
  return (
    <div className="relative">
      {picture}
      <button
        type="button"
        aria-label={studio.label}
        title={studio.label}
        disabled={studio.busy}
        onClick={() => studio.open(src)}
        className="absolute right-1 bottom-1 rounded-md bg-black/55 p-1 text-white opacity-80 hover:bg-black/70 hover:opacity-100 disabled:opacity-40"
      >
        <Sparkles size={13} aria-hidden />
      </button>
    </div>
  );
}

interface Props {
  message: MessageDto;
  isLastAssistant: boolean;
  busy: boolean;
  onRegenerate: (id: number) => void;
  /** [0.0.7 P6] With the attachments (by attachment id) the new message keeps. */
  onEditResend: (message: MessageDto, newText: string, attachmentIds: number[]) => void;
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
  // [0.0.4 search] An answer's `[n]` are checked against the chat's stored sources.
  const citations = useCitations(m.role === "assistant");
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(m.text);
  // [0.0.7 P6] The attachments the edited message keeps (by attachment id): all of them at
  // first; each can be removed in the edit box.
  const images = m.imageIds ?? [];
  const documents = m.documents ?? [];
  const mentions = m.mentions ?? [];
  const [kept, setKept] = useState<number[]>([]);
  const [copied, setCopied] = useState(false);
  // [0.1.0 B11] *Open in Studio* on its way, and why the last one did not add the picture.
  const [opening, setOpening] = useState(false);
  const [notInStudio, setNotInStudio] = useState<string | null>(null);

  /** Puts the attached picture `src` (a `data:` URI) in Studio's Pictures tab, and shows Studio;
   *  says here why when it cannot. */
  async function openInStudio(src: string) {
    const picture = pictureOf(src, STUDIO_CHAT_LINK.pictureName);
    if (!picture) return;
    setOpening(true);
    setNotInStudio(null);
    try {
      const why = await useStudio.getState().openInStudio(picture);
      if (why) setNotInStudio(STUDIO_CHAT_LINK.notAdded(why));
    } catch (e) {
      setNotInStudio(STUDIO_CHAT_LINK.notAdded(toErrorInfo(e).message));
    } finally {
      setOpening(false);
    }
  }

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
              // [0.0.7 P6a review] A box opened before a reply started stays open: it sends once
              // the reply is done (the app writes one at a time), keeping what was written.
              if (props.busy) return;
              const text = draft.trim();
              // With attachments left, the text may be empty (a message with only an image).
              if (text || kept.length > 0) props.onEditResend(m, text, kept);
              setEditing(false);
            }}
          >
            {kept.length > 0 && (
              <div aria-label="Attachments" className="mb-1.5 flex flex-wrap items-center justify-end gap-2">
                {images.map(
                  (id, i) =>
                    kept.includes(id) && (
                      <div key={id} className="relative">
                        <AttachedImage id={id} small />
                        <button
                          type="button"
                          aria-label={`Remove image ${i + 1}`}
                          onClick={() => setKept((old) => old.filter((k) => k !== id))}
                          className="absolute -top-1.5 -right-1.5 rounded-full bg-bg p-0.5 shadow"
                        >
                          <X size={12} aria-hidden />
                        </button>
                      </div>
                    ),
                )}
                {documents.map(
                  (d) =>
                    kept.includes(d.id) && (
                      <span key={d.id} className="flex items-center gap-1 rounded-md border border-border px-2 py-1 text-xs">
                        <FileText size={12} aria-hidden /> {d.name}
                        <button
                          type="button"
                          aria-label={`Remove ${d.name}`}
                          onClick={() => setKept((old) => old.filter((k) => k !== d.id))}
                        >
                          <X size={11} aria-hidden />
                        </button>
                      </span>
                    ),
                )}
                {mentions.map(
                  (mention) =>
                    kept.includes(mention.attachmentId) && (
                      <span
                        key={mention.attachmentId}
                        className="flex items-center gap-1 rounded-md border border-border bg-bg-subtle px-2 py-0.5 font-mono text-xs"
                      >
                        <AtSign size={11} className="text-accent" aria-hidden />
                        {mention.label}
                        <button
                          type="button"
                          aria-label={`Remove mention ${mention.label}`}
                          onClick={() => setKept((old) => old.filter((k) => k !== mention.attachmentId))}
                        >
                          <X size={11} aria-hidden />
                        </button>
                      </span>
                    ),
                )}
              </div>
            )}
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
              <button
                type="submit"
                disabled={props.busy || (!draft.trim() && kept.length === 0)}
                className="rounded-md bg-accent px-3 py-1 text-accent-fg disabled:opacity-50"
              >
                Send
              </button>
            </div>
          </form>
        ) : (
          <div className="flex max-w-[85%] flex-col items-end gap-2">
            {(m.imageIds ?? []).length > 0 && (
              <div className="flex flex-wrap justify-end gap-2">
                {m.imageIds.map((id, i) => (
                  <AttachedImage
                    key={id}
                    id={id}
                    studio={{
                      label: STUDIO_CHAT_LINK.openInStudio(i + 1, m.imageIds.length),
                      busy: opening,
                      open: (src) => void openInStudio(src),
                    }}
                  />
                ))}
              </div>
            )}
            {notInStudio && (
              <p role="status" className="text-xs text-warning">
                {notInStudio}
              </p>
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
            {(m.mentions ?? []).length > 0 && (
              <ul aria-label="Mentions" className="flex flex-wrap justify-end gap-1">
                {(m.mentions ?? []).map((mention) => (
                  <li
                    key={mention.attachmentId}
                    className="flex items-center gap-1 rounded-md border border-border bg-bg-subtle px-2 py-0.5 font-mono text-xs"
                    title={
                      mention.kind === "file"
                        ? `${mention.path}${mention.truncated ? " (first part sent)" : ""}`
                        : `${mention.kind}, lines ${mention.startLine}-${mention.endLine} of ${mention.path}`
                    }
                  >
                    <AtSign size={11} className="text-accent" aria-hidden />
                    {mention.label}
                  </li>
                ))}
              </ul>
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
                setKept([...images, ...documents.map((d) => d.id), ...mentions.map((x) => x.attachmentId)]);
                setEditing(true);
              }}
              className="text-fg-muted hover:text-fg disabled:opacity-30"
            >
              <Pencil size={13} aria-hidden />
            </button>
          )}
          {/* [0.0.4 pinned] Pins this message's stored text for the chat. */}
          {!editing && m.text && <PinMessageButton messageId={m.id} />}
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
          <Markdown text={m.text} citations={citations} />
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
