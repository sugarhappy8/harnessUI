import { useEffect, useRef, useState } from "react";
import { ArrowUp, Square } from "lucide-react";

export function Composer(props: {
  busy: boolean;
  disabledReason: string | null;
  onSend: (text: string) => void;
  onStop: () => void;
}) {
  const [text, setText] = useState("");
  const ref = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    function onFocus() {
      ref.current?.focus();
    }
    window.addEventListener("app:focus-input", onFocus);
    return () => window.removeEventListener("app:focus-input", onFocus);
  }, []);

  function submit() {
    const t = text.trim();
    if (!t || props.busy || props.disabledReason) return;
    props.onSend(t);
    setText("");
  }

  return (
    <div className="border-t border-border p-3">
      <div className="mx-auto flex max-w-3xl items-end gap-2 rounded-lg border border-border bg-bg px-3 py-2 focus-within:border-accent">
        <label className="sr-only" htmlFor="composer">
          Message
        </label>
        <textarea
          id="composer"
          ref={ref}
          value={text}
          onChange={(e) => setText(e.target.value)}
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
            disabled={!text.trim() || props.disabledReason !== null}
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
