import { createContext, isValidElement, memo, useContext, useMemo, useRef, useState, type ReactNode } from "react";
import { Check, Copy } from "lucide-react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeHighlight from "rehype-highlight";
import { AlertDialog } from "radix-ui";
import { openUrl } from "@tauri-apps/plugin-opener";
import { isTauri } from "../lib/ipc";
import { CITATION_ELEMENT, remarkCitations } from "./sources/citations";

/** Shows a link's real destination and opens it in the system browser only after
 *  the user confirms (ARCHITECTURE.md 13.2). Model output never navigates the app. */
export function SafeLink(props: { href?: string; children?: React.ReactNode }) {
  const [open, setOpen] = useState(false);
  let host = "";
  let display = props.href ?? "";
  let allowed: boolean;
  try {
    const url = new URL(props.href ?? "");
    allowed = url.protocol === "https:" || url.protocol === "http:";
    host = url.hostname; // URL() already converts non-ASCII hosts to punycode.
    display = url.href;
  } catch {
    allowed = false;
  }
  if (!allowed) return <span className="underline decoration-dotted">{props.children}</span>;
  return (
    <AlertDialog.Root open={open} onOpenChange={setOpen}>
      <AlertDialog.Trigger asChild>
        <button type="button" className="text-accent underline">
          {props.children}
        </button>
      </AlertDialog.Trigger>
      <AlertDialog.Portal>
        <AlertDialog.Overlay className="dialog-overlay fixed inset-0" />
        <AlertDialog.Content className="fixed top-1/2 left-1/2 w-[32rem] max-w-[90vw] -translate-x-1/2 -translate-y-1/2 rounded-2xl border border-border bg-surface p-5 shadow-pop">
          <AlertDialog.Title className="mb-2 font-semibold">Open this link?</AlertDialog.Title>
          <AlertDialog.Description className="mb-1 text-sm text-fg-muted">
            It opens in your web browser. Host:
          </AlertDialog.Description>
          <p className="mb-2 font-mono text-base font-semibold break-all">{host}</p>
          <p className="mb-4 font-mono text-xs break-all text-fg-muted">{display}</p>
          <div className="flex justify-end gap-2">
            <AlertDialog.Cancel className="rounded-md border border-border px-3 py-1.5 text-sm">
              Cancel
            </AlertDialog.Cancel>
            <AlertDialog.Action
              className="rounded-md bg-accent px-3 py-1.5 text-sm text-accent-fg"
              onClick={() => {
                if (isTauri()) void openUrl(display);
              }}
            >
              Open in browser
            </AlertDialog.Action>
          </div>
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}

/** Images in model output load only from data: or blob: URLs, so a model cannot leak
 *  data through an image request. Anything else is shown as text. */
function SafeImage(props: { src?: string | Blob; alt?: string }) {
  const src = typeof props.src === "string" ? props.src : "";
  if (src.startsWith("data:image/") || src.startsWith("blob:")) {
    return <img src={src} alt={props.alt ?? ""} className="max-w-full rounded" />;
  }
  return <span className="text-fg-muted">[image not loaded: {props.alt || "remote image"}]</span>;
}

/** A fenced code block: its language, and a button that copies the code as written. */
function CodeBlock(props: { children?: ReactNode }) {
  const ref = useRef<HTMLPreElement>(null);
  const [copied, setCopied] = useState(false);
  const child = props.children;
  const className = isValidElement<{ className?: string }>(child) ? (child.props.className ?? "") : "";
  const language = /language-([\w+#.-]+)/.exec(className)?.[1] ?? null;
  async function copy() {
    const text = ref.current?.textContent ?? "";
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
    <div className="my-3 overflow-hidden rounded-xl border border-border bg-bg-subtle">
      <div className="flex items-center justify-between border-b border-border py-1 pr-1.5 pl-3 text-xs text-fg-muted">
        <span className="font-mono">{language ?? "code"}</span>
        <button
          type="button"
          onClick={() => void copy()}
          className="flex items-center gap-1 rounded-md px-1.5 py-0.5 hover:bg-bg-muted hover:text-fg"
        >
          {copied ? <Check size={12} aria-hidden /> : <Copy size={12} aria-hidden />}
          {copied ? "Copied" : "Copy code"}
        </button>
      </div>
      <pre ref={ref} className="overflow-x-auto p-3 font-mono text-[13px] leading-relaxed">
        {props.children}
      </pre>
    </div>
  );
}

/** What an answer's citations are checked against: the sources a web search stored for
 *  the chat (their numbers, with title and site for the tooltip), and what a click does. */
export interface Citations {
  sources: ReadonlyMap<number, { title: string; site: string }>;
  onOpen: (n: number) => void;
}

const CitationContext = createContext<Citations | null>(null);

/** One `[n]` of an answer. With a source of that number: a small chip that opens it.
 *  Without one: the text as it was written, marked as pointing nowhere. */
function Citation(props: { n: number; children?: ReactNode }) {
  const citations = useContext(CitationContext);
  const source = citations?.sources.get(props.n);
  if (!citations || !source) {
    return (
      <span className="underline decoration-dotted decoration-fg-muted underline-offset-2" title="No such source" data-citation-missing={props.n}>
        {props.children}
      </span>
    );
  }
  const label = source.title ? `${source.title} — ${source.site}` : source.site;
  return (
    <button
      type="button"
      data-citation={props.n}
      title={label}
      aria-label={`Source ${props.n}: ${label}`}
      onClick={() => citations.onOpen(props.n)}
      className="mx-0.5 inline-flex h-[1.35em] min-w-[1.35em] items-center justify-center rounded-md border border-border bg-bg-muted px-1 align-baseline text-[0.72em] leading-none font-medium text-accent tabular-nums hover:border-accent"
    >
      {props.n}
    </button>
  );
}

const components: Components = {
  a: ({ href, children }) => <SafeLink href={href}>{children}</SafeLink>,
  // Only the citation plugin makes this element (raw HTML in an answer stays text).
  [CITATION_ELEMENT]: (props: { children?: ReactNode; "data-cite"?: string }) => {
    const n = Number(props["data-cite"]);
    return Number.isInteger(n) && n > 0 ? <Citation n={n}>{props.children}</Citation> : <>{props.children}</>;
  },
  img: ({ src, alt }) => <SafeImage src={src} alt={alt} />,
  pre: ({ children }) => <CodeBlock>{children}</CodeBlock>,
  code: ({ children, className }) => (
    <code className={(className ?? "") + " rounded-md bg-bg-muted px-1 py-0.5 font-mono text-[13px]"}>
      {children}
    </code>
  ),
  table: ({ children }) => (
    <div className="my-2 overflow-x-auto">
      <table className="overflow-hidden rounded-lg border border-border text-sm">{children}</table>
    </div>
  ),
  th: ({ children }) => (
    <th className="border-b border-border bg-bg-subtle px-3 py-1.5 text-left font-medium">{children}</th>
  ),
  td: ({ children }) => <td className="border-t border-border px-3 py-1.5">{children}</td>,
  ul: ({ children }) => <ul className="my-2 list-disc pl-6">{children}</ul>,
  ol: ({ children }) => <ol className="my-2 list-decimal pl-6">{children}</ol>,
  p: ({ children }) => <p className="my-2 first:mt-0 last:mb-0">{children}</p>,
  h1: ({ children }) => <h1 className="mt-3 mb-2 text-lg font-semibold">{children}</h1>,
  h2: ({ children }) => <h2 className="mt-3 mb-2 text-base font-semibold">{children}</h2>,
  h3: ({ children }) => <h3 className="mt-3 mb-1 font-semibold">{children}</h3>,
  blockquote: ({ children }) => (
    <blockquote className="my-2 border-l-2 border-accent/50 pl-3 text-fg-muted">{children}</blockquote>
  ),
};

// Built once: react-markdown runs its plugins on every render, which happens on each
// streamed chunk. Only fenced blocks that name a language are highlighted (no guessing).
const highlight = rehypeHighlight({ detect: false });

/** Code highlighting as tokens (ARCHITECTURE.md 14, 18): the highlighter adds hast
 *  spans with hljs-* classes, which react-markdown renders as React elements, so no
 *  HTML string is ever built. An unknown language stays plain text, and a highlighter
 *  failure leaves the code plain instead of breaking the message. */
function safeHighlight(): ReturnType<typeof rehypeHighlight> {
  return (tree, file) => {
    try {
      highlight(tree, file);
    } catch {
      // The code stays plain text.
    }
  };
}

/** Markdown rendered to React elements. Raw HTML in the source is shown as text,
 *  never parsed (react-markdown's default; no rehype-raw).
 *
 *  With `citations` (an answer in a chat whose sources are known), `[n]` in running text
 *  becomes a chip for source n, or is marked when the chat has no such source. Without
 *  it, `[n]` is the plain text it is. */
export const Markdown = memo(function Markdown(props: { text: string; citations?: Citations }) {
  const citations = props.citations;
  const remarkPlugins = useMemo(() => {
    if (!citations) return [remarkGfm];
    const known = (n: number) => citations.sources.has(n);
    return [remarkGfm, [remarkCitations, { known }] as [typeof remarkCitations, { known: (n: number) => boolean }]];
  }, [citations]);
  return (
    <CitationContext.Provider value={citations ?? null}>
      <ReactMarkdown
        remarkPlugins={remarkPlugins}
        rehypePlugins={[safeHighlight]}
        components={components}
        skipHtml={false}
      >
        {props.text}
      </ReactMarkdown>
    </CitationContext.Provider>
  );
});
