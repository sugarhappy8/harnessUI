import { useEffect, useMemo, useRef } from "react";
import { BadgeCheck, CheckCircle2, CircleAlert, Globe } from "lucide-react";
import { useApp } from "../../store/app";
import { SafeLink } from "../Markdown";
import { checkQuotes, type QuoteCheck } from "../sources/quotes";
import { startSources, useSources } from "../sources/state";
import type { Source } from "../sources/types";

startSources();

function when(ms: number): string {
  return new Date(ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

/** What the model was given of a source: its title, the search service's summary and the
 *  text of the page. The quote check looks in exactly this. */
function givenText(source: Source): string {
  return [source.title, source.snippet, source.excerpt].filter((part) => part !== "").join("\n\n");
}

/** Quotes that answers attribute to a source, each with whether the retrieved text has it. */
function QuoteResults(props: { check: QuoteCheck }) {
  const { cited, quotes } = props.check;
  if (cited === 0) return <p className="text-xs text-fg-muted">Not cited in an answer yet.</p>;
  if (quotes.length === 0) {
    return (
      <p className="text-xs text-fg-muted">
        Cited in {cited === 1 ? "one answer" : `${cited} answers`}, without a quoted passage to check.
      </p>
    );
  }
  return (
    <ul className="space-y-1.5">
      {quotes.map((q) => (
        <li key={`${q.messageId}:${q.quote}`} className="flex items-start gap-2 text-xs" data-quote-found={q.found}>
          {q.found ? (
            <CheckCircle2 size={14} className="mt-0.5 shrink-0 text-success" aria-hidden />
          ) : (
            <CircleAlert size={14} className="mt-0.5 shrink-0 text-warning" aria-hidden />
          )}
          <span className="min-w-0">
            <span className="break-words">“{q.quote}”</span>
            <span className={"ml-1.5 font-medium " + (q.found ? "text-success" : "text-warning")}>
              {q.found ? "quote found" : "quote not found in the retrieved text"}
            </span>
          </span>
        </li>
      ))}
    </ul>
  );
}

function SourceCard(props: { source: Source; check: QuoteCheck; focused: boolean }) {
  const s = props.source;
  const ref = useRef<HTMLElement>(null);
  useEffect(() => {
    if (props.focused) ref.current?.scrollIntoView?.({ block: "start", behavior: "smooth" });
  }, [props.focused]);
  return (
    <article
      ref={ref}
      data-source={s.n}
      aria-label={`Source ${s.n}`}
      className={
        "rounded-xl border bg-surface p-3 text-sm shadow-soft " + (props.focused ? "border-accent" : "border-border")
      }
    >
      <header className="flex items-start gap-2">
        <span className="mt-0.5 inline-flex h-6 min-w-6 shrink-0 items-center justify-center rounded-md bg-bg-muted px-1.5 text-xs font-medium text-accent tabular-nums">
          {s.n}
        </span>
        <div className="min-w-0 flex-1">
          <h3 className="font-medium break-words">{s.title || s.site}</h3>
          <p className="flex items-center gap-1 text-xs text-fg-muted">
            <Globe size={12} aria-hidden />
            <span className="break-all">{s.site}</span>
          </p>
        </div>
      </header>
      {(s.official || s.versionNote) && (
        <p className="mt-2 flex flex-wrap gap-1.5">
          {s.official && (
            <span className="inline-flex items-center gap-1 rounded-md bg-bg-muted px-1.5 py-0.5 text-xs text-success">
              <BadgeCheck size={12} aria-hidden />
              official documentation
            </span>
          )}
          {s.versionNote && (
            <span className="rounded-md bg-bg-muted px-1.5 py-0.5 text-xs text-fg-muted">{s.versionNote}</span>
          )}
        </p>
      )}
      <dl className="mt-2 space-y-0.5 text-xs text-fg-muted">
        <div className="flex gap-1.5">
          <dt className="shrink-0">Address</dt>
          <dd className="min-w-0 font-mono break-all text-fg">{s.url}</dd>
        </div>
        {s.queries.length > 0 && (
          <div className="flex gap-1.5">
            <dt className="shrink-0">{s.queries.length === 1 ? "Found by the search" : "Found by the searches"}</dt>
            <dd className="min-w-0 break-words text-fg">{s.queries.join(" · ")}</dd>
          </div>
        )}
        <div className="flex gap-1.5">
          <dt className="shrink-0">Page read</dt>
          <dd className="text-fg">
            {s.fetchedAt !== null ? when(s.fetchedAt) : "not read: the model was given only the search service's summary"}
          </dd>
        </div>
        {s.contentSha256 && (
          <div className="flex gap-1.5">
            <dt className="shrink-0" title="A fingerprint of the page's text when it was read. The same text always gives the same one.">
              Text hash (SHA-256)
            </dt>
            <dd className="min-w-0 font-mono break-all text-fg">{s.contentSha256}</dd>
          </div>
        )}
      </dl>
      {s.snippet && (
        <section className="mt-2">
          <h4 className="text-xs text-fg-muted">Summary from the search service, as given to the model</h4>
          <p className="mt-0.5 rounded-lg bg-bg p-2 text-xs whitespace-pre-wrap break-words">{s.snippet}</p>
        </section>
      )}
      {s.excerpt && (
        <section className="mt-2">
          <h4 className="text-xs text-fg-muted">Text of the page, as given to the model</h4>
          <pre className="mt-0.5 max-h-72 overflow-auto rounded-lg bg-bg p-2 font-sans text-xs whitespace-pre-wrap break-words">
            {s.excerpt}
          </pre>
        </section>
      )}
      <section className="mt-2" aria-label="Check quotes">
        <h4 className="mb-1 text-xs text-fg-muted">Check quotes</h4>
        <QuoteResults check={props.check} />
      </section>
      <p className="mt-2 text-xs">
        <SafeLink href={s.url}>Open in browser</SafeLink>
      </p>
    </article>
  );
}

/** The "Sources" tab of the panel beside the chat: every page a web search retrieved for
 *  the chat, by the number answers cite it with, with exactly the text the model was
 *  given of it. All of it is text from the web and is shown as plain text. */
export function SourcesTab(props: { chatId: number }) {
  const sources = useSources((s) => s.byChat[props.chatId]);
  const focus = useSources((s) => s.focus);
  const detail = useApp((s) => s.detail);
  useEffect(() => {
    void useSources.getState().load(props.chatId);
  }, [props.chatId]);
  const answers = useMemo(
    () =>
      (detail && detail.chat.id === props.chatId ? detail.messages : [])
        .filter((m) => m.role === "assistant" && m.text !== "")
        .map((m) => ({ id: m.id, text: m.text })),
    [detail, props.chatId],
  );
  const checks = useMemo(
    () => new Map((sources ?? []).map((s) => [s.n, checkQuotes(answers, s.n, givenText(s))])),
    [sources, answers],
  );
  if (!sources || sources.length === 0) {
    return (
      <p className="p-6 text-sm text-fg-muted" data-chat={props.chatId}>
        Pages a web search retrieved for this chat appear here, with the text the answer relied on.
      </p>
    );
  }
  const focused = focus && focus.chatId === props.chatId ? focus : null;
  return (
    <div className="space-y-3 p-3" data-chat={props.chatId}>
      <p className="px-1 text-xs text-fg-muted">
        {sources.length === 1 ? "One source" : `${sources.length} sources`}. An answer cites a source by its number.
        What is shown here is what the model was given, so a citation can be checked against it.
      </p>
      {sources.map((s) => (
        <SourceCard
          key={focused?.n === s.n ? `${s.n}:${focused.seq}` : s.n}
          source={s}
          check={checks.get(s.n) ?? { cited: 0, quotes: [] }}
          focused={focused?.n === s.n}
        />
      ))}
    </div>
  );
}
