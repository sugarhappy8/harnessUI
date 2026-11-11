import { useLayoutEffect, useRef } from "react";

/**
 * The source of an artifact as plain text, to read or to change. Tab indents by two
 * spaces. So that the keyboard can still leave the box, Escape lets the next Tab move on.
 */
export function SourceEditor(props: { value: string; onChange: (value: string) => void; label: string }) {
  const box = useRef<HTMLTextAreaElement>(null);
  /** Where the caret goes after React has written the new text. */
  const caret = useRef<number | null>(null);
  const leaving = useRef(false);

  useLayoutEffect(() => {
    if (caret.current === null || !box.current) return;
    box.current.setSelectionRange(caret.current, caret.current);
    caret.current = null;
  });

  return (
    <textarea
      ref={box}
      aria-label={props.label}
      aria-describedby="artifact-editor-keys"
      value={props.value}
      spellCheck={false}
      autoCapitalize="off"
      autoCorrect="off"
      onChange={(event) => props.onChange(event.target.value)}
      onBlur={() => {
        leaving.current = false;
      }}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          leaving.current = true;
          return;
        }
        if (event.key !== "Tab" || event.shiftKey || event.ctrlKey || event.metaKey || event.altKey) return;
        if (leaving.current) return;
        event.preventDefault();
        const el = event.currentTarget;
        const start = el.selectionStart;
        caret.current = start + 2;
        props.onChange(`${el.value.slice(0, start)}  ${el.value.slice(el.selectionEnd)}`);
      }}
      className="h-full min-h-40 w-full resize-none border-0 bg-bg p-3 font-mono text-[13px] leading-relaxed outline-none"
    />
  );
}
