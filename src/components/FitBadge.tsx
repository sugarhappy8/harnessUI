import type { FitResult, Verdict } from "../lib/types";
import { FIT_WORDS, fitLine } from "../lib/words";

const COLORS: Record<Verdict, string> = {
  green: "bg-success",
  yellow: "bg-warning",
  red: "bg-danger",
};

/**
 * A fit as a colour dot and a word (never colour alone), with one plain line after the word
 * when given: Compare's and the repository page's verdict, and Studio's local fit words.
 */
export function FitDot(props: { verdict: Verdict; word: string; line?: string }) {
  return (
    <p className="mt-0.5 flex items-start gap-1.5 text-xs text-fg-muted">
      <span className={`mt-1 inline-block h-2 w-2 shrink-0 rounded-full ${COLORS[props.verdict]}`} aria-hidden />
      <span>
        {props.line === undefined ? (
          <span className="font-medium text-fg">{props.word}</span>
        ) : (
          <>
            <span className="font-medium text-fg">{props.word}.</span> {props.line}
          </>
        )}
      </span>
    </p>
  );
}

/**
 * The fit verdict as a colour dot, a word (never colour alone) and one plain line from
 * words.ts (compare-fit-words): the backend's 0.0.4 sentence is never shown.
 */
export function FitBadge(props: { fit: FitResult }) {
  return <FitDot verdict={props.fit.verdict} word={FIT_WORDS[props.fit.verdict]} line={fitLine(props.fit)} />;
}
