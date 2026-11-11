import { useEffect, useRef, useState } from "react";
import { ArrowUp, FileText, Paperclip, Square, X } from "lucide-react";
import type { DocumentInput, ImageInput } from "../lib/types";

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

export function Composer(props: {
  busy: boolean;
  disabledReason: string | null;
  onSend: (text: string, images: ImageInput[], documents: DocumentInput[]) => void;
  onStop: () => void;
}) {
  const [text, setText] = useState("");
  const [images, setImages] = useState<ImageInput[]>([]);
  const [documents, setDocuments] = useState<DocumentInput[]>([]);
  const [imageError, setImageError] = useState<string | null>(null);
  const ref = useRef<HTMLTextAreaElement>(null);
  const fileInput = useRef<HTMLInputElement>(null);

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

  function submit() {
    const t = text.trim();
    if ((!t && images.length === 0 && documents.length === 0) || props.busy || props.disabledReason) return;
    props.onSend(t, images, documents);
    setText("");
    setImages([]);
    setDocuments([]);
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
      <div className="composer mx-auto flex max-w-3xl items-end gap-2 rounded-2xl px-2.5 py-2">
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
          onChange={(e) => setText(e.target.value)}
          onPaste={(e) => {
            const files = [...e.clipboardData.files];
            if (files.length > 0) {
              e.preventDefault();
              void addFiles(files);
            }
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              submit();
            }
          }}
          rows={Math.min(8, Math.max(1, text.split("\n").length))}
          placeholder={props.disabledReason ?? "Ask anything, or describe a task"}
          className="chat-text max-h-48 flex-1 resize-none self-center bg-transparent py-1 outline-none placeholder:text-fg-muted focus-visible:outline-none"
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
            onClick={submit}
            disabled={(!text.trim() && images.length === 0 && documents.length === 0) || props.disabledReason !== null}
            className="btn-primary rounded-xl p-2.5"
            aria-label="Send"
            title="Send (Enter)"
          >
            <ArrowUp size={15} strokeWidth={2.5} aria-hidden />
          </button>
        )}
      </div>
      <p className="mx-auto mt-1.5 max-w-3xl px-2 text-center text-[11px] text-fg-muted">
        Enter to send · Shift+Enter for a new line · the model asks before it changes anything
      </p>
    </div>
  );
}
