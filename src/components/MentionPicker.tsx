import { Braces, FileText } from "lucide-react";
import type { MentionHit, MentionInput } from "../lib/types";

/** A mention chosen in the picker: its text in the message and what the app reads. */
export interface MentionChip {
  label: string;
  kind: string;
  input: MentionInput;
}

/** The `@query` being typed just before the caret, if any. An `@` inside a word (an e-mail
 * address) does not start one. */
export function mentionQueryAt(text: string, caret: number): { start: number; query: string } | null {
  const before = text.slice(0, caret);
  const match = /(^|\s)@([^\s@]*)$/.exec(before);
  const query = match?.[2];
  if (query === undefined) return null;
  return { start: before.length - query.length - 1, query };
}

/** Replaces the `@query` from `start` to `caret` with `@label ` and returns the new text and
 * caret position. */
export function insertMention(text: string, start: number, caret: number, label: string) {
  const token = `@${label} `;
  const rest = text.slice(caret).replace(/^\S*/, "").replace(/^ /, "");
  return { text: text.slice(0, start) + token + rest, caret: start + token.length };
}

function isTokenEnd(text: string, at: number): boolean {
  const next = text[at];
  if (next === undefined || /\s/.test(next)) return true;
  if (/[,;:!?)\]}"']/.test(next)) return true;
  // A full stop ends a sentence, not a name: only when nothing follows it.
  const after = text[at + 1];
  return next === "." && (after === undefined || /\s/.test(after));
}

/** Where `@label` stands in `text` as a whole token, or -1. */
export function tokenIndex(text: string, label: string): number {
  const token = `@${label}`;
  let from = 0;
  for (;;) {
    const i = text.indexOf(token, from);
    if (i < 0) return -1;
    const startOk = i === 0 || /\s/.test(text[i - 1] ?? "");
    if (startOk && isTokenEnd(text, i + token.length)) return i;
    from = i + 1;
  }
}

/** `text` without the first `@label` token (and one space after it). */
export function removeToken(text: string, label: string): string {
  const i = tokenIndex(text, label);
  if (i < 0) return text;
  let end = i + label.length + 1;
  if (text[end] === " ") end += 1;
  return text.slice(0, i) + text.slice(end);
}

/** The chips whose token is still in the text: only those are sent. A typed or pasted
 * `@name` with no chip is plain text. */
export function chipsInText(chips: MentionChip[], text: string): MentionChip[] {
  return chips.filter((c) => tokenIndex(text, c.label) >= 0);
}

export function chipFor(hit: MentionHit): MentionChip {
  const input: MentionInput = { path: hit.path };
  if (hit.symbol) {
    input.symbol = hit.symbol;
    input.kind = hit.kind;
    if (hit.container) input.container = hit.container;
    if (hit.line !== undefined) input.line = hit.line;
  }
  return { label: hit.label, kind: hit.kind, input };
}

/** The file part of a label: `src/lib.rs#parse` → `src/lib.rs`. */
function fileOf(hit: MentionHit): string {
  const i = hit.label.indexOf("#");
  return i < 0 ? hit.label : hit.label.slice(0, i);
}

export const optionId = (listId: string, i: number) => `${listId}-option-${i}`;

/** The list of files and definitions shown above the message box while an `@` is typed. */
export function MentionPicker(props: {
  id: string;
  hits: MentionHit[];
  active: number;
  indexing: boolean;
  onChoose: (hit: MentionHit) => void;
  onHover: (index: number) => void;
}) {
  if (props.hits.length === 0) {
    return (
      <div
        role="status"
        className="absolute right-0 bottom-full left-0 mb-2 rounded-xl border border-border bg-surface px-3 py-2 text-xs text-fg-muted shadow-soft"
      >
        {props.indexing ? "Looking through the folders…" : "No matching files or definitions in the approved folders."}
      </div>
    );
  }
  return (
    <ul
      id={props.id}
      role="listbox"
      aria-label="Files and definitions"
      className="absolute right-0 bottom-full left-0 z-20 mb-2 max-h-72 overflow-y-auto rounded-xl border border-border bg-surface py-1 shadow-soft"
    >
      {props.hits.map((hit, i) => {
        const isSymbol = hit.kind !== "file";
        const selected = i === props.active;
        return (
          <li
            key={`${hit.label}:${hit.line ?? 0}`}
            id={optionId(props.id, i)}
            role="option"
            aria-selected={selected}
            // Keep the focus in the message box.
            onMouseDown={(e) => e.preventDefault()}
            onMouseEnter={() => props.onHover(i)}
            onClick={() => props.onChoose(hit)}
            className={`flex cursor-pointer items-center gap-2 px-3 py-1.5 text-sm ${
              selected ? "bg-bg-muted" : "hover:bg-bg-subtle"
            }`}
          >
            {isSymbol ? (
              <Braces size={13} className="shrink-0 text-accent" aria-hidden />
            ) : (
              <FileText size={13} className="shrink-0 text-fg-muted" aria-hidden />
            )}
            {isSymbol ? (
              <>
                <span className="truncate font-mono text-xs">{hit.label.slice(hit.label.indexOf("#") + 1)}</span>
                <span className="shrink-0 text-xs text-fg-muted">{hit.kind}</span>
                <span className="ml-auto truncate text-xs text-fg-muted">
                  {fileOf(hit)}:{hit.line}
                </span>
              </>
            ) : (
              <span className="truncate font-mono text-xs">{hit.label}</span>
            )}
          </li>
        );
      })}
    </ul>
  );
}
