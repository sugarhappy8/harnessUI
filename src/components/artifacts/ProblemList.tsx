import { AlertTriangle } from "lucide-react";
import { fixRequest, problemLine } from "./messages";
import type { Problem } from "./types";

/**
 * What a preview reported, above the preview. The lines come from a page the model wrote:
 * they are shown as text and nothing else. "Ask the model to fix these" writes them into
 * the message box; nothing is sent until the user sends it.
 */
export function ProblemList(props: { title: string; id: number; version: number; problems: Problem[] }) {
  const { problems } = props;
  if (problems.length === 0) return null;
  return (
    <section
      aria-label="Problems in the preview"
      className="enter-fade border-b border-border bg-surface px-3 py-2 text-xs"
    >
      <div className="mb-1 flex items-center gap-1.5 font-medium text-warning">
        <AlertTriangle size={13} aria-hidden />
        {problems.length === 1 ? "The preview reported a problem" : `The preview reported ${problems.length} problems`}
      </div>
      <ul className="mb-2 max-h-28 space-y-0.5 overflow-y-auto font-mono text-[11px] text-fg-muted">
        {problems.map((p, i) => (
          <li key={i} className="break-words whitespace-pre-wrap">
            {problemLine(p)}
          </li>
        ))}
      </ul>
      <button
        type="button"
        onClick={() =>
          window.dispatchEvent(
            new CustomEvent("app:compose", { detail: fixRequest(props.title, props.id, props.version, problems) }),
          )
        }
        className="rounded-lg border border-border px-2.5 py-1 font-medium hover:border-border-strong hover:bg-bg-muted"
      >
        Ask the model to fix these
      </button>
    </section>
  );
}
