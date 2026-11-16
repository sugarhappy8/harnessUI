import { useState } from "react";
import { X } from "lucide-react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import type { ErrorInfo } from "../lib/types";
import { isTauri } from "../lib/ipc";
import { useInputDelay } from "../lib/useInputDelay";

/** A plain-language error with a next step and the technical details behind
 *  "Copy details". Never a raw stack trace (ARCHITECTURE.md 15). */
export function ErrorBanner(props: {
  error: ErrorInfo;
  onDismiss?: () => void;
  /** "notice": the app stopped on purpose and nothing is broken (a limit was reached). */
  tone?: "error" | "notice";
  /** The one thing to do about it, as a button. With `delayMs`, clicks on it are ignored for
   *  that long after it appears and after the app brings its window back (`useInputDelay`). */
  action?: { label: string; onClick: () => void; delayMs?: number };
}) {
  const notice = props.tone === "notice";
  const [copied, setCopied] = useState(false);
  async function copy() {
    const text = `${props.error.message}\n${props.error.nextStep}\ncode: ${props.error.code}\n\n${props.error.details}`;
    try {
      if (isTauri()) await writeText(text);
      else await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      setCopied(false);
    }
  }
  return (
    <div
      role={notice ? "status" : "alert"}
      className={
        "enter flex items-start gap-3 rounded-xl border px-3.5 py-2.5 text-sm shadow-soft " +
        (notice ? "border-warning/40 bg-warning/10" : "border-danger/40 bg-danger/10")
      }
    >
      <div className="flex-1">
        <p className="font-medium">{props.error.message}</p>
        <p className="text-fg-muted">{props.error.nextStep}</p>
      </div>
      {props.action &&
        (props.action.delayMs === undefined ? (
          <button
            type="button"
            className="btn-primary rounded-lg px-3 py-1 text-sm font-medium"
            onClick={props.action.onClick}
          >
            {props.action.label}
          </button>
        ) : (
          <DelayedAction label={props.action.label} onClick={props.action.onClick} delayMs={props.action.delayMs} />
        ))}
      {props.error.details && (
        <button type="button" className="text-xs underline" onClick={() => void copy()}>
          {copied ? "Copied" : "Copy details"}
        </button>
      )}
      {props.onDismiss && (
        <button type="button" aria-label="Dismiss" onClick={props.onDismiss}>
          <X size={14} aria-hidden />
        </button>
      )}
    </div>
  );
}

/** The action button with an input delay, which starts when the button appears. */
function DelayedAction(props: { label: string; onClick: () => void; delayMs: number }) {
  const { armed, armedRef } = useInputDelay(props.delayMs);
  return (
    <button
      type="button"
      aria-disabled={!armed}
      className="btn-primary rounded-lg px-3 py-1 text-sm font-medium aria-disabled:opacity-50"
      onClick={() => armedRef.current && props.onClick()}
    >
      {props.label}
    </button>
  );
}
