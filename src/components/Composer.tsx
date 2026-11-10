import { useEffect, useRef, useState } from "react";
import { ArrowUp, ImagePlus, Square, X } from "lucide-react";
import type { ImageInput } from "../lib/types";

const IMAGE_TYPES = ["image/png", "image/jpeg", "image/gif", "image/webp"];
const MAX_IMAGE_BYTES = 20 * 1024 * 1024;
const MAX_IMAGES = 8;

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
  onSend: (text: string, images: ImageInput[]) => void;
  onStop: () => void;
}) {
  const [text, setText] = useState("");
  const [images, setImages] = useState<ImageInput[]>([]);
  const [imageError, setImageError] = useState<string | null>(null);
  const ref = useRef<HTMLTextAreaElement>(null);
  const fileInput = useRef<HTMLInputElement>(null);

  async function addFiles(files: Iterable<File>) {
    setImageError(null);
    const list = [...files].filter((f) => f.type.startsWith("image/"));
    try {
      const read = await Promise.all(list.map(readImage));
      setImages((old) => [...old, ...read].slice(0, MAX_IMAGES));
    } catch (err) {
      setImageError(err instanceof Error ? err.message : String(err));
    }
  }

  useEffect(() => {
    function onFocus() {
      ref.current?.focus();
    }
    window.addEventListener("app:focus-input", onFocus);
    return () => window.removeEventListener("app:focus-input", onFocus);
  }, []);

  function submit() {
    const t = text.trim();
    if ((!t && images.length === 0) || props.busy || props.disabledReason) return;
    props.onSend(t, images);
    setText("");
    setImages([]);
  }

  return (
    <div
      className="border-t border-border p-3"
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
      {(images.length > 0 || imageError) && (
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
          {imageError && <span className="text-xs text-danger">{imageError}</span>}
        </div>
      )}
      <div className="mx-auto flex max-w-3xl items-end gap-2 rounded-lg border border-border bg-bg px-3 py-2 focus-within:border-accent">
        <button
          type="button"
          aria-label="Attach an image"
          title="Attach an image"
          onClick={() => fileInput.current?.click()}
          className="rounded-md p-1.5 text-fg-muted hover:bg-bg-muted hover:text-fg"
        >
          <ImagePlus size={16} aria-hidden />
        </button>
        <input
          ref={fileInput}
          type="file"
          accept={IMAGE_TYPES.join(",")}
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
          placeholder={props.disabledReason ?? "Message (Enter to send, Shift+Enter for a new line)"}
          className="max-h-48 flex-1 resize-none bg-transparent outline-none"
        />
        {props.busy ? (
          <button
            type="button"
            onClick={props.onStop}
            className="rounded-md bg-danger p-2 text-white"
            aria-label="Stop"
            title="Stop"
          >
            <Square size={14} aria-hidden />
          </button>
        ) : (
          <button
            type="button"
            onClick={submit}
            disabled={(!text.trim() && images.length === 0) || props.disabledReason !== null}
            className="rounded-md bg-accent p-2 text-accent-fg disabled:opacity-40"
            aria-label="Send"
          >
            <ArrowUp size={14} aria-hidden />
          </button>
        )}
      </div>
    </div>
  );
}
