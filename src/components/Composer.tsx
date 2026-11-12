import { useEffect, useId, useRef, useState } from "react";
import { ArrowUp, AtSign, FileText, Paperclip, Square, X } from "lucide-react";
import type { DocumentInput, ImageInput, MentionHit, MentionInput } from "../lib/types";
import { resolveMentions, searchMentions } from "../lib/ipc";
import { ApprovalHint } from "./AllowAllButton";
import {
  type MentionChip,
  MentionPicker,
  chipFor,
  chipsInText,
  insertMention,
  mentionQueryAt,
  optionId,
  removeToken,
} from "./MentionPicker";

/** Mentions in one message at most (the app refuses more). */
const MAX_MENTIONS = 8;
/** Wait after a keystroke before searching. */
const SEARCH_DELAY_MS = 80;
/** Wait before asking again while the folders are still being indexed. */
const INDEXING_RETRY_MS = 400;

const IMAGE_TYPES = ["image/png", "image/jpeg", "image/gif", "image/webp"];
const MAX_IMAGE_BYTES = 20 * 1024 * 1024;
const MAX_IMAGES = 8;
const MAX_DOCUMENT_BYTES = 32 * 1024 * 1024;
/** Text-like files and PDFs the composer accepts besides images. */
const DOCUMENT_EXTENSIONS = [
  ".pdf", ".txt", ".md", ".markdown", ".csv", ".tsv", ".json", ".yaml", ".yml", ".toml", ".xml", ".html", ".log",
  ".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".go", ".java", ".c", ".h", ".cpp", ".cs", ".rb", ".sh", ".ps1", ".sql",
];

function readDocument(file: File): Promise<DocumentInput> {
  return new Promise((resolve, reject) => {
    if (file.size > MAX_DOCUMENT_BYTES) {
      reject(new Error(`${file.name} is larger than 32 MB.`));
      return;
    }
    const reader = new FileReader();
    reader.onerror = () => reject(new Error(`${file.name} could not be read.`));
    reader.onload = () => {
      const url = String(reader.result);
      resolve({ fileName: file.name || "document", data: url.slice(url.indexOf(",") + 1) });
    };
    reader.readAsDataURL(file);
  });
}

/** Reads an image file into the shape the backend expects. */
export function readImage(file: File): Promise<ImageInput> {
  return new Promise((resolve, reject) => {
    if (!IMAGE_TYPES.includes(file.type)) {
      reject(new Error(`${file.name} is not a PNG, JPEG, GIF or WebP image.`));
      return;
    }
    if (file.size > MAX_IMAGE_BYTES) {
      reject(new Error(`${file.name} is larger than 20 MB.`));
      return;
    }
    const reader = new FileReader();
    reader.onerror = () => reject(new Error(`${file.name} could not be read.`));
    reader.onload = () => {
      const url = String(reader.result);
      const comma = url.indexOf(",");
      resolve({ mime: file.type, fileName: file.name || "image", data: url.slice(comma + 1) });
    };
    reader.readAsDataURL(file);
  });
}

/** The message of an error from the app or the browser. */
function messageOf(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "object" && err !== null && "message" in err) return String(err.message);
  return String(err);
}

export function Composer(props: {
  busy: boolean;
  disabledReason: string | null;
  /** The open chat; its approved folders are what `@` can mention. */
  chatId?: number | null;
  onSend: (text: string, images: ImageInput[], documents: DocumentInput[], mentions: MentionInput[]) => void;
  onStop: () => void;
}) {
  const chatId = props.chatId ?? null;
  const [text, setText] = useState("");
  const [images, setImages] = useState<ImageInput[]>([]);
  const [documents, setDocuments] = useState<DocumentInput[]>([]);
  const [imageError, setImageError] = useState<string | null>(null);
  const ref = useRef<HTMLTextAreaElement>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  // Mentions: the chips chosen so far, and the picker while an `@` is typed.
  const [chips, setChips] = useState<MentionChip[]>([]);
  const [picking, setPicking] = useState<{ start: number; query: string } | null>(null);
  const [hits, setHits] = useState<MentionHit[]>([]);
  const [indexing, setIndexing] = useState(false);
  const [active, setActive] = useState(0);
  const [checking, setChecking] = useState(false);
  const listId = useId();
  const pickerOpen = picking !== null;

  // Search as the query changes; ask again while the folders are still being indexed.
  useEffect(() => {
    if (!picking) return;
    let alive = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const ask = (delay: number) => {
      timer = setTimeout(() => {
        searchMentions(chatId, picking.query, 20)
          .then((found) => {
            if (!alive) return;
            setHits(found.hits);
            setIndexing(found.indexing);
            setActive((a) => Math.min(a, Math.max(0, found.hits.length - 1)));
            if (found.indexing) ask(INDEXING_RETRY_MS);
          })
          .catch(() => {
            if (alive) setHits([]);
          });
      }, delay);
    };
    ask(SEARCH_DELAY_MS);
    return () => {
      alive = false;
      if (timer) clearTimeout(timer);
    };
  }, [picking, chatId]);

  /** Opens, updates or closes the picker for the text and caret now in the box. */
  function track(value: string, caret: number) {
    const found = mentionQueryAt(value, caret);
    setPicking((old) => (found && old && old.start === found.start && old.query === found.query ? old : found));
    if (!found) {
      setHits([]);
      setActive(0);
    }
  }

  function choose(hit: MentionHit) {
    const box = ref.current;
    if (!picking || !box) return;
    const caret = box.selectionStart;
    const next = insertMention(text, picking.start, caret, hit.label);
    setText(next.text);
    setChips((old) => {
      const kept = chipsInText(old, next.text).filter((c) => c.label !== hit.label);
      return [...kept, chipFor(hit)].slice(0, MAX_MENTIONS);
    });
    setPicking(null);
    setHits([]);
    setActive(0);
    requestAnimationFrame(() => {
      box.focus();
      box.setSelectionRange(next.caret, next.caret);
    });
  }

  function removeChip(label: string) {
    setChips((old) => old.filter((c) => c.label !== label));
    setText((old) => removeToken(old, label));
  }

  async function addFiles(files: Iterable<File>) {
    setImageError(null);
    const all = [...files];
    const imgs = all.filter((f) => f.type.startsWith("image/"));
    const docs = all.filter((f) => !f.type.startsWith("image/"));
    try {
      const read = await Promise.all(imgs.map(readImage));
      setImages((old) => [...old, ...read].slice(0, MAX_IMAGES));
      const readDocs = await Promise.all(docs.map(readDocument));
      setDocuments((old) => [...old, ...readDocs].slice(0, MAX_IMAGES));
    } catch (err) {
      setImageError(err instanceof Error ? err.message : String(err));
    }
  }

  useEffect(() => {
    function onFocus() {
      ref.current?.focus();
    }
    // A suggestion was chosen on the welcome screen: write it into the box, send nothing.
    function onCompose(event: Event) {
      const detail: unknown = (event as CustomEvent<unknown>).detail;
      if (typeof detail !== "string") return;
      setText(detail);
      requestAnimationFrame(() => {
        const box = ref.current;
        if (!box) return;
        box.focus();
        box.setSelectionRange(detail.length, detail.length);
      });
    }
    window.addEventListener("app:focus-input", onFocus);
    window.addEventListener("app:compose", onCompose);
    return () => {
      window.removeEventListener("app:focus-input", onFocus);
      window.removeEventListener("app:compose", onCompose);
    };
  }, []);

  async function submit() {
    const t = text.trim();
    if ((!t && images.length === 0 && documents.length === 0) || props.busy || props.disabledReason || checking) return;
    // Only chips whose `@name` is still in the text are sent.
    const sent = chipsInText(chips, t);
    const mentions = sent.map((c) => c.input);
    if (mentions.length > 0 && chatId !== null) {
      // Checked before the box is cleared, so a mention that cannot be read keeps the text.
      setChecking(true);
      try {
        const checks = await resolveMentions(chatId, mentions);
        const failed = checks.find((c) => c.error);
        if (failed?.error) {
          setImageError(failed.error);
          return;
        }
      } catch (err) {
        setImageError(messageOf(err));
        return;
      } finally {
        setChecking(false);
      }
    }
    props.onSend(t, images, documents, mentions);
    setText("");
    setImages([]);
    setDocuments([]);
    setChips([]);
    setPicking(null);
    setImageError(null);
  }

  return (
    <div
      className="px-4 pt-1 pb-3"
      onDragOver={(e) => {
        if (e.dataTransfer.types.includes("Files")) e.preventDefault();
      }}
      onDrop={(e) => {
        if (e.dataTransfer.files.length > 0) {
          e.preventDefault();
          void addFiles(e.dataTransfer.files);
        }
      }}
    >
      {(images.length > 0 || documents.length > 0 || imageError) && (
        <div className="mx-auto mb-2 flex max-w-3xl flex-wrap items-center gap-2">
          {images.map((img, i) => (
            <div key={`${img.fileName}-${i}`} className="relative">
              <img
                src={`data:${img.mime};base64,${img.data}`}
                alt={img.fileName}
                className="h-16 w-16 rounded-md border border-border object-cover"
              />
              <button
                type="button"
                aria-label={`Remove ${img.fileName}`}
                onClick={() => setImages((old) => old.filter((_, j) => j !== i))}
                className="absolute -top-1.5 -right-1.5 rounded-full bg-bg p-0.5 shadow"
              >
                <X size={12} aria-hidden />
              </button>
            </div>
          ))}
          {documents.map((d, i) => (
            <span key={`${d.fileName}-${i}`} className="flex items-center gap-1 rounded-md border border-border px-2 py-1 text-xs">
              <FileText size={12} aria-hidden /> {d.fileName}
              <button
                type="button"
                aria-label={`Remove ${d.fileName}`}
                onClick={() => setDocuments((old) => old.filter((_, j) => j !== i))}
              >
                <X size={11} aria-hidden />
              </button>
            </span>
          ))}
          {imageError && <span className="text-xs text-danger">{imageError}</span>}
        </div>
      )}
      <div className="relative mx-auto max-w-3xl">
        {pickerOpen && (
          <MentionPicker
            id={listId}
            hits={hits}
            active={active}
            indexing={indexing}
            onChoose={choose}
            onHover={setActive}
          />
        )}
        <div className="composer flex items-end gap-2 rounded-2xl px-2.5 py-2">
          <button
            type="button"
            aria-label="Attach files"
            title="Attach images, PDFs or text files"
            onClick={() => fileInput.current?.click()}
            className="rounded-xl p-2 text-fg-muted hover:bg-bg-muted hover:text-fg"
          >
            <Paperclip size={17} aria-hidden />
          </button>
          <input
            ref={fileInput}
            type="file"
            accept={[...IMAGE_TYPES, ...DOCUMENT_EXTENSIONS].join(",")}
            multiple
            hidden
            onChange={(e) => {
              if (e.target.files) void addFiles(e.target.files);
              e.target.value = "";
            }}
          />
          <label className="sr-only" htmlFor="composer">
            Message
          </label>
          <textarea
            id="composer"
            ref={ref}
            value={text}
            role="combobox"
            aria-autocomplete="list"
            aria-haspopup="listbox"
            aria-expanded={pickerOpen && hits.length > 0}
            aria-controls={pickerOpen && hits.length > 0 ? listId : undefined}
            aria-activedescendant={pickerOpen && hits.length > 0 ? optionId(listId, active) : undefined}
            onChange={(e) => {
              setText(e.target.value);
              track(e.target.value, e.target.selectionStart);
              // A chip whose `@name` was deleted from the text is dropped.
              setChips((old) => chipsInText(old, e.target.value));
            }}
            onSelect={(e) => track(e.currentTarget.value, e.currentTarget.selectionStart)}
            onBlur={() => setPicking(null)}
            onPaste={(e) => {
              const files = [...e.clipboardData.files];
              if (files.length > 0) {
                e.preventDefault();
                void addFiles(files);
              }
            }}
            onKeyDown={(e) => {
              if (pickerOpen) {
                if (e.key === "Escape") {
                  e.preventDefault();
                  setPicking(null);
                  return;
                }
                if (hits.length > 0) {
                  if (e.key === "ArrowDown") {
                    e.preventDefault();
                    setActive((a) => (a + 1) % hits.length);
                    return;
                  }
                  if (e.key === "ArrowUp") {
                    e.preventDefault();
                    setActive((a) => (a - 1 + hits.length) % hits.length);
                    return;
                  }
                  if ((e.key === "Enter" || e.key === "Tab") && !e.shiftKey && !e.nativeEvent.isComposing) {
                    e.preventDefault();
                    const hit = hits[active] ?? hits[0];
                  if (hit) choose(hit);
                    return;
                  }
                }
              }
              if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                void submit();
              }
            }}
            rows={Math.min(8, Math.max(1, text.split("\n").length))}
            placeholder={props.disabledReason ?? "Ask anything, or type @ to mention a file"}
            className="chat-text max-h-48 flex-1 resize-none self-center bg-transparent py-1 outline-none placeholder:text-fg-muted"
          />
          {props.busy ? (
            <button
              type="button"
              onClick={props.onStop}
              className="enter-fade rounded-xl bg-danger p-2.5 text-white shadow-soft hover:brightness-110"
              aria-label="Stop"
              title="Stop"
            >
              <Square size={14} fill="currentColor" aria-hidden />
            </button>
          ) : (
            <button
              type="button"
              onClick={() => void submit()}
              disabled={(!text.trim() && images.length === 0 && documents.length === 0) || props.disabledReason !== null || checking}
              className="btn-primary rounded-xl p-2.5"
              aria-label="Send"
              title="Send (Enter)"
            >
              <ArrowUp size={15} strokeWidth={2.5} aria-hidden />
            </button>
          )}
        </div>
      </div>
      {chips.length > 0 && (
        <ul aria-label="Mentions" className="mx-auto mt-1.5 flex max-w-3xl flex-wrap gap-1.5 px-1">
          {chips.map((c) => (
            <li
              key={c.label}
              className="flex items-center gap-1 rounded-md border border-border bg-bg-subtle px-2 py-0.5 font-mono text-xs"
              title={c.kind === "file" ? c.input.path : `${c.kind} in ${c.input.path}`}
            >
              <AtSign size={11} className="text-accent" aria-hidden />
              {c.label}
              <button type="button" aria-label={`Remove mention ${c.label}`} onClick={() => removeChip(c.label)}>
                <X size={11} aria-hidden />
              </button>
            </li>
          ))}
        </ul>
      )}
      <p className="mx-auto mt-1.5 max-w-3xl px-2 text-center text-[11px] text-fg-muted">
        Enter to send · Shift+Enter for a new line · <ApprovalHint />
      </p>
    </div>
  );
}
