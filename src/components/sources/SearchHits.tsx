import { useEffect, type ReactNode } from "react";
import { BookOpen } from "lucide-react";
import { useApp } from "../../store/app";
import type { ToolCallView } from "../ToolCallCard";
import { openSource } from "./useCitations";
import { useSources } from "./state";

/**
 * The numbers of the sources a `web_search` result lists. Only its first line is read,
 * which the app writes itself (`Sources [1] [2] for: …`); everything after it is text
 * from the web and is not trusted to say what was found.
 */
export function hitNumbers(resultText: string): number[] {
  const line = resultText.split("\n", 1)[0] ?? "";
  const match = /^Sources ((?:\[\d{1,4}\] )+)for: /.exec(line);
  if (!match) return [];
  return [...match[1]!.matchAll(/\[(\d{1,4})\]/g)].map((m) => Number(m[1]));
}

/** The number of the source a `read_source` result is about (its first line: `[n] …`). */
export function readNumber(resultText: string): number | null {
  const match = /^\[(\d{1,4})\] /.exec(resultText);
  return match ? Number(match[1]) : null;
}

/** Under the card of a search: the sources it found, each a button that shows the source
 *  in the panel beside the chat. Titles and sites are text from the web: plain text. */
function SearchHits(props: { numbers: number[] }) {
  const chatId = useApp((s) => s.activeChatId);
  const sources = useSources((s) => (chatId === null ? undefined : s.byChat[chatId]));
  useEffect(() => {
    if (chatId !== null) useSources.getState().ensure(chatId);
  }, [chatId]);
  return (
    <ul className="space-y-0.5" aria-label="Sources found">
      {props.numbers.map((n) => {
        const source = sources?.find((s) => s.n === n);
        return (
          <li key={n}>
            <button
              type="button"
              disabled={chatId === null}
              onClick={() => chatId !== null && openSource(chatId, n)}
              title="Show this source"
              className="flex w-full items-center gap-2 rounded-md px-1 py-0.5 text-left text-xs hover:bg-bg-muted"
            >
              <span className="inline-flex h-5 min-w-5 shrink-0 items-center justify-center rounded bg-bg-muted px-1 font-medium text-accent tabular-nums">
                {n}
              </span>
              <span className="min-w-0 flex-1 truncate">{source ? source.title || source.site : `Source ${n}`}</span>
              {source && <span className="shrink-0 text-fg-muted">{source.site}</span>}
              <BookOpen size={12} className="shrink-0 text-fg-muted" aria-hidden />
            </button>
          </li>
        );
      })}
    </ul>
  );
}

/** For the registry in `toolcards.tsx`: nothing until the search has found something. */
export function searchHitsExtra(call: ToolCallView): ReactNode {
  const numbers = hitNumbers(call.text);
  return numbers.length > 0 ? <SearchHits numbers={numbers} /> : null;
}

/** For `read_source`: the source that was read. */
export function readSourceExtra(call: ToolCallView): ReactNode {
  if (call.status !== "ok") return null;
  const n = readNumber(call.text);
  return n === null ? null : <SearchHits numbers={[n]} />;
}
