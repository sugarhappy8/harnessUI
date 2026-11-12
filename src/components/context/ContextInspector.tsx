import { useEffect, useRef, useState } from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { ChevronDown, ChevronRight, Copy, Eye } from "lucide-react";
import { toErrorInfo, useApp } from "../../store/app";
import * as api from "./api";
import type { ContextSection, ContextView } from "./types";

/** The most characters of one section drawn at once. "Copy all" has everything. */
export const DRAWN_CHARS = 20_000;

function grouped(n: number): string {
  return n.toLocaleString("en-US");
}

function inApp(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

async function copy(text: string): Promise<void> {
  if (inApp()) await writeText(text);
  else await navigator.clipboard.writeText(text);
}

/** The whole view as one plain text, for "Copy all". */
export function copyAllText(view: ContextView): string {
  const lines: string[] = [view.model];
  if (view.totals) {
    lines.push(`Total: about ${grouped(view.totals.tokens)} of ${grouped(view.totals.nCtx)} tokens`);
  }
  if (view.refusal) lines.push(`Would not start: ${view.refusal}`);
  lines.push(`Left out next: ${view.nextDrop}`);
  for (const s of view.sections) {
    const tokens = s.tokens === null ? "" : ` (about ${grouped(s.tokens)} tokens)`;
    lines.push("", `=== ${s.title}${tokens}`, s.detail);
    if (s.text) lines.push("", s.text);
  }
  return lines.join("\n");
}

function SectionRow(props: { section: ContextSection }) {
  const s = props.section;
  const [open, setOpen] = useState(false);
  const empty = s.text === "";
  const cut = s.text.length > DRAWN_CHARS;
  const panel = `context-section-${s.id}`;
  return (
    <li className="rounded-lg border border-border bg-surface" data-section={s.id}>
      <button
        type="button"
        aria-expanded={open}
        aria-controls={panel}
        disabled={empty}
        onClick={() => setOpen(!open)}
        className="flex w-full items-start gap-2 px-2.5 py-2 text-left text-sm disabled:cursor-default"
      >
        <span className="mt-0.5 text-fg-muted">
          {empty ? (
            <ChevronRight size={14} aria-hidden className="opacity-30" />
          ) : open ? (
            <ChevronDown size={14} aria-hidden />
          ) : (
            <ChevronRight size={14} aria-hidden />
          )}
        </span>
        <span className="min-w-0 flex-1">
          <span className="font-medium">{s.title}</span>
          <span className="block text-xs break-words text-fg-muted" data-section-detail>
            {s.detail}
          </span>
        </span>
        {s.tokens !== null && (
          <span className="shrink-0 text-xs text-fg-muted tabular-nums" data-section-tokens>
            {grouped(s.tokens)} tokens
          </span>
        )}
      </button>
      {open && (
        <div id={panel} className="border-t border-border px-2.5 py-2">
          <pre className="max-h-96 overflow-auto rounded-md bg-bg p-2 font-mono text-xs whitespace-pre-wrap break-words">
            {cut ? s.text.slice(0, DRAWN_CHARS) : s.text}
          </pre>
          {cut && (
            <p className="mt-1 text-xs text-fg-muted">
              Cut here for display: {grouped(s.text.length - DRAWN_CHARS)} more characters. Copy all has everything.
            </p>
          )}
        </div>
      )}
    </li>
  );
}

function Bar(props: { used: number; of: number; mark?: number }) {
  const pct = (n: number) => `${Math.min(100, Math.max(0, (n / Math.max(1, props.of)) * 100))}%`;
  return (
    <div className="relative h-1.5 overflow-hidden rounded-full bg-bg-muted" aria-hidden>
      <div className="h-full rounded-full bg-accent" style={{ width: pct(props.used) }} />
      {props.mark !== undefined && (
        <div className="absolute top-0 h-full w-px bg-fg-muted" style={{ left: pct(props.mark) }} />
      )}
    </div>
  );
}

/** Exactly what the next request of the chat would contain, section by section, with
 *  token estimates, the total against the model's context and what would be left out
 *  next. Read again when the chat, its tools, its pins or its task change, or a turn ends. */
export function ContextInspector(props: { chatId: number }) {
  const { chatId } = props;
  const [view, setView] = useState<ContextView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [version, setVersion] = useState(0);
  const latest = useRef(0);
  const running = useApp((s) => s.generation?.chatId === chatId);
  const lastMessage = useApp((s) => {
    const m = s.detail?.chat.id === chatId ? s.detail.messages : undefined;
    return m && m.length > 0 ? m[m.length - 1]!.id : null;
  });

  // Read the view; a newer read wins over an older one.
  useEffect(() => {
    if (running) return;
    const seq = ++latest.current;
    api.inspectContext(chatId).then(
      (v) => {
        if (seq === latest.current) {
          setView(v);
          setError(null);
        }
      },
      (err: unknown) => {
        if (seq === latest.current) setError(toErrorInfo(err).message);
      },
    );
  }, [chatId, running, lastMessage, version]);

  // What else changes the next request.
  useEffect(() => {
    let gone = false;
    const offs: (() => void)[] = [];
    const again = () => setVersion((v) => v + 1);
    const keep = (p: Promise<() => void>) =>
      void p.then((off) => {
        if (gone) off();
        else offs.push(off);
      });
    keep(api.onPinsChanged((changed) => (changed === null || changed === chatId) && again()));
    keep(api.onTaskChanged((changed) => changed === chatId && again()));
    keep(api.onToolServersChanged(again));
    const tools = (e: Event) => {
      if ((e as CustomEvent<number>).detail === chatId) again();
    };
    window.addEventListener("chat-tools-changed", tools);
    return () => {
      gone = true;
      offs.forEach((off) => off());
      window.removeEventListener("chat-tools-changed", tools);
    };
  }, [chatId]);

  const totals = view?.totals ?? null;
  return (
    <section className="space-y-3 border-t border-border p-4" aria-labelledby="inspector-heading">
      <header className="flex items-center gap-2">
        <h2 id="inspector-heading" className="flex items-center gap-1.5 text-sm font-medium">
          <Eye size={14} aria-hidden className="text-accent" />
          What the model gets next
        </h2>
        <div className="flex-1" />
        <button
          type="button"
          disabled={!view}
          onClick={() => {
            if (!view) return;
            void copy(copyAllText(view)).then(
              () => {
                setCopied(true);
                setTimeout(() => setCopied(false), 1500);
              },
              (err: unknown) => setError(toErrorInfo(err).message),
            );
          }}
          className="flex items-center gap-1 rounded-lg px-2 py-1 text-xs text-fg-muted hover:bg-bg-muted hover:text-fg disabled:opacity-40"
        >
          <Copy size={13} aria-hidden />
          {copied ? "Copied" : "Copy all"}
        </button>
      </header>
      {running && <p className="text-xs text-fg-muted">A reply is being written. This is read again when it ends.</p>}
      {error && (
        <p className="text-sm text-danger" data-inspector-error>
          {error}
        </p>
      )}
      {view && (
        <>
          <p className="text-xs text-fg-muted" data-inspector-model>
            {view.model}
          </p>
          {totals && (
            <div className="space-y-2 rounded-lg border border-border bg-surface p-2.5 text-xs" data-inspector-totals>
              <p>
                <span className="font-medium tabular-nums">
                  About {grouped(totals.tokens)} of {grouped(totals.nCtx)} tokens
                </span>{" "}
                <span className="text-fg-muted">
                  ({totals.exact ? "counted by the model server" : "estimated"}; up to {grouped(totals.budget)} can be used
                  before the reply, and room is made from {grouped(totals.trigger)})
                </span>
              </p>
              <Bar used={totals.tokens} of={totals.nCtx} mark={totals.trigger} />
              <p className="text-fg-muted tabular-nums">
                Pinned instructions: {grouped(totals.pinned.used)} of {grouped(totals.pinned.allowed)} tokens allowed
              </p>
              {totals.tools && (
                <p className="text-fg-muted tabular-nums">
                  Fixed part without pins: {grouped(totals.tools.used)} of {grouped(totals.tools.allowed)} tokens allowed
                  with tools on
                </p>
              )}
            </div>
          )}
          {view.refusal && (
            <p className="text-sm text-danger" data-inspector-refusal>
              The next message would not be sent: {view.refusal}
            </p>
          )}
          <p className="text-xs" data-inspector-next>
            <span className="font-medium">Left out next: </span>
            {view.nextDrop}
          </p>
          {view.notes.map((n) => (
            <p key={n} className="text-xs text-fg-muted">
              {n}
            </p>
          ))}
          <ul className="space-y-1.5" aria-label="Sections of the next request">
            {view.sections.map((s) => (
              <SectionRow key={s.id} section={s} />
            ))}
          </ul>
        </>
      )}
    </section>
  );
}
