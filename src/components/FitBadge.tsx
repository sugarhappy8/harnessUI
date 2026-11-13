import type { FitResult } from "../lib/types";
import { FIT_WORDS, fitLine } from "../lib/words";

const COLORS: Record<FitResult["verdict"], string> = {
  green: "bg-success",
  yellow: "bg-warning",
  red: "bg-danger",
};

/**
 * The fit verdict as a colour dot, a word (never colour alone) and one plain line from
 * words.ts (compare-fit-words): the backend's 0.0.4 sentence is never shown.
 */
export function FitBadge(props: { fit: FitResult }) {
  return (
    <p className="mt-0.5 flex items-start gap-1.5 text-xs text-fg-muted">
      <span className={`mt-1 inline-block h-2 w-2 shrink-0 rounded-full ${COLORS[props.fit.verdict]}`} aria-hidden />
      <span>
        <span className="font-medium text-fg">{FIT_WORDS[props.fit.verdict]}.</span> {fitLine(props.fit)}
      </span>
    </p>
  );
}
