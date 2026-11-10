import { memo, useState } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { AlertDialog } from "radix-ui";
import { openUrl } from "@tauri-apps/plugin-opener";
import { isTauri } from "../lib/ipc";

/** Shows a link's real destination and opens it in the system browser only after
 *  the user confirms (ARCHITECTURE.md 13.2). Model output never navigates the app. */
function SafeLink(props: { href?: string; children?: React.ReactNode }) {
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
        <AlertDialog.Overlay className="fixed inset-0 bg-black/40" />
        <AlertDialog.Content className="fixed top-1/2 left-1/2 w-[32rem] max-w-[90vw] -translate-x-1/2 -translate-y-1/2 rounded-lg border border-border bg-bg p-5 shadow-xl">
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

const components: Components = {
  a: ({ href, children }) => <SafeLink href={href}>{children}</SafeLink>,
  img: ({ src, alt }) => <SafeImage src={src} alt={alt} />,
  pre: ({ children }) => (
    <pre className="my-2 overflow-x-auto rounded-md bg-bg-muted p-3 text-[13px]">{children}</pre>
  ),
  code: ({ children, className }) => (
    <code className={(className ?? "") + " rounded bg-bg-muted px-1 font-mono text-[13px]"}>
      {children}
    </code>
  ),
  table: ({ children }) => (
    <div className="my-2 overflow-x-auto">
      <table className="border-collapse text-sm">{children}</table>
    </div>
  ),
  th: ({ children }) => <th className="border border-border px-2 py-1 text-left">{children}</th>,
  td: ({ children }) => <td className="border border-border px-2 py-1">{children}</td>,
  ul: ({ children }) => <ul className="my-2 list-disc pl-6">{children}</ul>,
  ol: ({ children }) => <ol className="my-2 list-decimal pl-6">{children}</ol>,
  p: ({ children }) => <p className="my-2 first:mt-0 last:mb-0">{children}</p>,
  h1: ({ children }) => <h1 className="mt-3 mb-2 text-lg font-semibold">{children}</h1>,
  h2: ({ children }) => <h2 className="mt-3 mb-2 text-base font-semibold">{children}</h2>,
  h3: ({ children }) => <h3 className="mt-3 mb-1 font-semibold">{children}</h3>,
  blockquote: ({ children }) => (
    <blockquote className="my-2 border-l-2 border-border pl-3 text-fg-muted">{children}</blockquote>
  ),
};

/** Markdown rendered to React elements. Raw HTML in the source is shown as text,
 *  never parsed (react-markdown's default; no rehype-raw). */
export const Markdown = memo(function Markdown(props: { text: string }) {
  return (
    <ReactMarkdown remarkPlugins={[remarkGfm]} components={components} skipHtml={false}>
      {props.text}
    </ReactMarkdown>
  );
});
