import type { FitResult } from "../lib/types";

const COLORS: Record<FitResult["verdict"], string> = {
  green: "bg-success",
  yellow: "bg-warning",
  red: "bg-danger",
};

const LABELS: Record<FitResult["verdict"], string> = {
  green: "Fits",
  yellow: "Fits with limits",
  red: "Too large",
};

/** The fit verdict as a colour dot, a word (never colour alone) and one sentence. */
export function FitBadge(props: { fit: FitResult }) {
  return (
    <p className="mt-0.5 flex items-start gap-1.5 text-xs text-fg-muted">
      <span className={`mt-1 inline-block h-2 w-2 shrink-0 rounded-full ${COLORS[props.fit.verdict]}`} aria-hidden />
      <span>
        <span className="font-medium text-fg">{LABELS[props.fit.verdict]}.</span> {props.fit.sentence}
      </span>
    </p>
  );
}
