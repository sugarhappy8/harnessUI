import { useState } from "react";
import { minimizeMainWindow } from "../lib/ipc";
import { TOOL_CARD_EXTRAS } from "./toolcards";
import { Ban, CheckCircle2, ChevronDown, ChevronRight, Hand, Loader2, Wrench, XCircle } from "lucide-react";

/** How much of a result is shown before "Show all". */
const RESULT_PREVIEW_CHARS = 4000;

export interface ToolCallView {
  callId: string;
  name: string;
  arguments: Record<string, unknown>;
  recovered: boolean;
  /** running, ok, error, denied, cancelled, refused, ... */
  status: string;
  text: string;
  decision?: string | null;
  durationMs?: number | null;
  progress?: string;
}

function argSummary(args: Record<string, unknown>): string {
  const first = ["path", "command", "query", "url", "name", "folder", "repo", "task"].find(
    (k) => typeof args[k] === "string",
  );
  if (first) return String(args[first]);
  const s = JSON.stringify(args);
  return s.length > 80 ? `${s.slice(0, 80)}…` : s;
}

function StatusIcon(props: { status: string }) {
  switch (props.status) {
    case "running":
      return <Loader2 size={14} className="animate-spin text-fg-muted" aria-label="Running" />;
    case "waiting":
      return <Hand size={14} className="text-warning" aria-label="Waiting for your answer" />;
    case "ok":
      return <CheckCircle2 size={14} className="text-success" aria-label="Done" />;
    case "denied":
    case "cancelled":
    case "refused":
      return <Ban size={14} className="text-fg-muted" aria-label={props.status} />;
    default:
      return <XCircle size={14} className="text-danger" aria-label="Failed" />;
  }
}

const STATUS_TEXT: Record<string, string> = {
  running: "Running",
  waiting: "Waiting for you",
  ok: "Done",
  error: "Failed",
  denied: "Denied",
  cancelled: "Cancelled",
  refused: "Not allowed",
};

/** The tool that hands a task to a helper (src-tauri/src/subagent.rs). */
const SUBAGENT_TOOL = "run_subagent";

/** The last line of a helper's progress that says something. */
function lastLine(progress: string): string {
  const lines = progress.split("\n").filter((l) => l.trim() !== "");
  return lines[lines.length - 1] ?? "";
}

/**
 * One tool call with its result. Arguments and results come from the model and from
 * tools, so they are shown as plain text only.
 *
 * A helper's card shows what the helper is doing right now under its header while it
 * works, without being opened: its tool calls, one line each, and its answer as it is
 * written. Opened, it shows all of that; when the helper is done, its answer.
 */
export function ToolCallCard(props: { call: ToolCallView }) {
  const c = props.call;
  const [open, setOpen] = useState(false);
  const [all, setAll] = useState(false);
  const result = c.text || c.progress || "";
  const shown = all ? result : result.slice(0, RESULT_PREVIEW_CHARS);
  const extra = TOOL_CARD_EXTRAS[c.name]?.(c) ?? null;
  const helperNow =
    c.name === SUBAGENT_TOOL && (c.status === "running" || c.status === "waiting") ? lastLine(c.progress ?? "") : "";
  return (
    <div className="enter overflow-hidden rounded-xl border border-border bg-surface text-sm shadow-soft">
      <button
        type="button"
        className="flex w-full items-center gap-2 px-3 py-2 text-left hover:bg-bg-muted/60"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        {open ? <ChevronDown size={14} aria-hidden /> : <ChevronRight size={14} aria-hidden />}
        <span className="rounded-md bg-bg-muted p-1 text-accent">
          <Wrench size={13} aria-hidden />
        </span>
        <span className="font-medium">{c.name}</span>
        <span className="min-w-0 flex-1 truncate font-mono text-xs text-fg-muted">{argSummary(c.arguments)}</span>
        <StatusIcon status={c.status} />
        <span className="text-xs text-fg-muted">{STATUS_TEXT[c.status] ?? c.status}</span>
      </button>
      {helperNow && !open && (
        <p className="truncate px-3 pb-1.5 pl-9 font-mono text-xs text-fg-muted" aria-live="off">
          {helperNow}
        </p>
      )}
      {extra && <div className="border-t border-border px-3 py-2">{extra}</div>}
      {open && (
        <div className="enter-fade space-y-2 border-t border-border px-3 py-2">
          {c.recovered && (
            <p className="text-xs text-warning">This call was recovered from the model&apos;s text.</p>
          )}
          <div>
            <div className="text-xs text-fg-muted">Arguments</div>
            <pre className="max-h-60 overflow-auto rounded bg-bg p-2 font-mono text-xs whitespace-pre-wrap break-all">
              {JSON.stringify(c.arguments, null, 2)}
            </pre>
          </div>
          {result && (
            <div>
              <div className="text-xs text-fg-muted">{c.status === "running" ? "Output so far" : "Result"}</div>
              <pre className="max-h-96 overflow-auto rounded bg-bg p-2 font-mono text-xs whitespace-pre-wrap break-all">
                {shown}
              </pre>
              {result.length > RESULT_PREVIEW_CHARS && !all && (
                <button type="button" className="text-xs text-accent" onClick={() => setAll(true)}>
                  Show all ({result.length.toLocaleString()} characters)
                </button>
              )}
            </div>
          )}
          {c.name.startsWith("computer_") && result.includes("this app's own window") && (
            <button
              type="button"
              onClick={() => void minimizeMainWindow()}
              className="rounded-md border border-border px-2 py-1 text-xs"
            >
              Minimise this window while the agent works
            </button>
          )}
          <div className="flex gap-3 text-xs text-fg-muted">
            {c.decision && <span>{c.decision}</span>}
            {c.durationMs != null && <span>{(c.durationMs / 1000).toFixed(1)} s</span>}
          </div>
        </div>
      )}
    </div>
  );
}
