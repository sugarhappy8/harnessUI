// Citations in an answer: `[n]` in running text, where n is the number of a source a web
// search stored for the chat.
//
// Model text is never turned into HTML for this. `splitCitations` cuts plain text into
// plain text and citations; `remarkCitations` does that for the text nodes of a parsed
// answer (never inside code or links) and marks each citation so that the Markdown
// component renders it as a React element.

/** A piece of running text: plain text, or one citation. */
export type Piece = string | { n: number; raw: string };

/** `[3]`, or a list in one bracket: `[1, 2]`. Numbers from 1 to 999, no leading zero. */
const BRACKET = /\[([1-9]\d{0,2}(?:\s*,\s*[1-9]\d{0,2}){0,5})\]/g;

/**
 * Cuts `text` into plain text and citations, in order.
 *
 * A single `[n]` is always a citation, also when the chat has no such source: where it is
 * shown decides between a link to the source and a mark that says there is none. A list
 * (`[1, 2]`) counts only when every number in it is a source; otherwise it is ordinary
 * text, like an array someone wrote out.
 */
export function splitCitations(text: string, known: (n: number) => boolean): Piece[] {
  const pieces: Piece[] = [];
  let last = 0;
  for (const match of text.matchAll(BRACKET)) {
    const numbers = match[1]!.split(",").map((part) => Number(part.trim()));
    if (numbers.length > 1 && !numbers.every(known)) continue;
    if (match.index > last) pieces.push(text.slice(last, match.index));
    for (const n of numbers) pieces.push({ n, raw: `[${n}]` });
    last = match.index + match[0].length;
  }
  if (last < text.length) pieces.push(text.slice(last));
  return pieces;
}

/** The numbers cited in `text`, in order (a list in one bracket counts as its numbers). */
export function citedNumbers(text: string): { n: number; index: number; end: number }[] {
  const found: { n: number; index: number; end: number }[] = [];
  for (const match of text.matchAll(BRACKET)) {
    for (const part of match[1]!.split(",")) {
      found.push({ n: Number(part.trim()), index: match.index, end: match.index + match[0].length });
    }
  }
  return found;
}

/** The part of a Markdown tree this file looks at. */
interface MdNode {
  type: string;
  value?: string;
  children?: MdNode[];
  label?: string | null;
  referenceType?: string;
  data?: { hName?: string; hProperties?: Record<string, string> };
}

/** The element a citation becomes. Markdown itself never produces a `cite` element (raw
 *  HTML in an answer is shown as text), so every `cite` the renderer sees is one of ours. */
export const CITATION_ELEMENT = "cite";

function citationNode(n: number, raw: string): MdNode {
  return { type: "text", value: raw, data: { hName: CITATION_ELEMENT, hProperties: { dataCite: String(n) } } };
}

function transform(node: MdNode, known: (n: number) => boolean): void {
  if (!node.children) return;
  const next: MdNode[] = [];
  for (const child of node.children) {
    if (child.type === "text" && typeof child.value === "string" && !child.data?.hName) {
      for (const piece of splitCitations(child.value, known)) {
        next.push(typeof piece === "string" ? { type: "text", value: piece } : citationNode(piece.n, piece.raw));
      }
      continue;
    }
    // `[1]` with a line `[1]: https://…` somewhere in the answer is a link in Markdown.
    // In an answer it is still a citation of source 1, not a link to wherever the model
    // pointed it.
    if (
      child.type === "linkReference" &&
      child.referenceType !== "full" &&
      typeof child.label === "string" &&
      /^[1-9]\d{0,2}$/.test(child.label)
    ) {
      next.push(citationNode(Number(child.label), `[${child.label}]`));
      continue;
    }
    // Text of a link stays the link's text; code has no child nodes at all.
    if (child.type !== "link" && child.type !== "linkReference") transform(child, known);
    next.push(child);
  }
  node.children = next;
}

/** The remark plugin: marks the citations in a parsed answer. */
export function remarkCitations(options: { known: (n: number) => boolean }) {
  return (tree: MdNode) => transform(tree, options.known);
}
