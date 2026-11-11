// The quote check of the Sources tab: does the text a source gave the model really hold
// what an answer quotes from it?
//
// Pure functions over plain text. Nothing here renders anything.
import { citedNumbers } from "./citations";

/** A quoted passage in an answer, with where it stands. */
export interface Quoted {
  quote: string;
  start: number;
  end: number;
}

/** Quotes shorter than this are words in quotation marks, not quotations. */
const MIN_QUOTE_CHARS = 20;
const MAX_QUOTE_CHARS = 1200;

/** Opening mark and the marks that may close it. */
const MARKS: [string, string][] = [
  ['"', '"'],
  ["“", "”"],
  ["„", "“”"],
  ["«", "»"],
];

/** `text` with its code (fenced blocks and inline spans) blanked out, same length. */
function withoutCode(text: string): string {
  const blank = (s: string) => s.replace(/[^\n]/g, " ");
  return text.replace(/```[\s\S]*?(?:```|$)/g, blank).replace(/`[^`\n]*`/g, blank);
}

/** The quoted passages of at least 20 characters in `text`. A quote does not cross a
 *  blank line, and quotation marks inside code are not quotes. */
export function quotedPassages(text: string): Quoted[] {
  const plain = withoutCode(text);
  const found: Quoted[] = [];
  let i = 0;
  while (i < plain.length) {
    const mark = MARKS.find(([open]) => plain[i] === open);
    if (!mark) {
      i += 1;
      continue;
    }
    let end = -1;
    for (let j = i + 1; j < plain.length && j - i <= MAX_QUOTE_CHARS + 1; j += 1) {
      if (plain[j] === "\n" && /^\s*\n/.test(plain.slice(j + 1))) break;
      if (mark[1].includes(plain[j]!)) {
        end = j;
        break;
      }
    }
    if (end < 0) {
      i += 1;
      continue;
    }
    const quote = text.slice(i + 1, end).trim();
    if (quote.length >= MIN_QUOTE_CHARS) found.push({ quote, start: i, end: end + 1 });
    i = end + 1;
  }
  return found;
}

/** Whether an answer cites source `n` (outside code). */
export function citedIn(text: string, n: number): boolean {
  return citedNumbers(withoutCode(text)).some((c) => c.n === n);
}

interface Group {
  numbers: number[];
  index: number;
  end: number;
}

/** Citations that stand together (`[1][2]`, `[1], [2]`, `[1, 2]`) as one group. */
function groups(text: string, from: number, to: number): Group[] {
  const out: Group[] = [];
  for (const c of citedNumbers(text.slice(from, to))) {
    const index = c.index + from;
    const end = c.end + from;
    const last = out[out.length - 1];
    if (last && /^[\s,;]*$/.test(text.slice(last.end, index))) {
      last.numbers.push(c.n);
      last.end = end;
    } else if (last && last.index === index) {
      last.numbers.push(c.n);
    } else {
      out.push({ numbers: [c.n], index, end });
    }
  }
  return out;
}

/**
 * The quotes of one answer that belong to source `n`. A quote belongs to the citation
 * that follows it before the next quote; if there is none, to the one before it; if
 * there is none either, to the nearest citation in the same paragraph.
 */
export function quotesFor(text: string, n: number): string[] {
  const plain = withoutCode(text);
  const out: string[] = [];
  // Paragraphs, with their place in the text.
  const paragraphs: { from: number; to: number }[] = [];
  let from = 0;
  for (const gap of plain.matchAll(/\n\s*\n/g)) {
    paragraphs.push({ from, to: gap.index });
    from = gap.index + gap[0].length;
  }
  paragraphs.push({ from, to: plain.length });
  const quotes = quotedPassages(text);
  for (const paragraph of paragraphs) {
    const inside = quotes.filter((q) => q.start >= paragraph.from && q.end <= paragraph.to);
    inside.forEach((quote, i) => {
      const previousEnd = i > 0 ? inside[i - 1]!.end : paragraph.from;
      const nextStart = i + 1 < inside.length ? inside[i + 1]!.start : paragraph.to;
      const after = groups(plain, quote.end, nextStart)[0];
      const before = groups(plain, previousEnd, quote.start).pop();
      const later = groups(plain, quote.end, paragraph.to)[0];
      const earlier = groups(plain, paragraph.from, quote.start).pop();
      const owner = after ?? before ?? later ?? earlier;
      if (owner?.numbers.includes(n)) out.push(quote.quote);
    });
  }
  return out;
}

/** Text as it is compared: one letter case, one kind of quotation mark and dash, no
 *  emphasis marks, white space as single spaces. */
function normal(text: string): string {
  return text
    .normalize("NFKC")
    .toLowerCase()
    .replace(/[‘’‚‛′]/g, "'")
    .replace(/[“”„‟«»]/g, '"')
    .replace(/[‐-―−]/g, "-")
    .replace(/…/g, "...")
    .replace(/[*_`]/g, "")
    .replace(/\s+/g, " ")
    .trim();
}

/**
 * Whether `quote` stands in `retrieved`, after both were normalised. A quote that leaves
 * something out (`...`, `[…]`) is found when each of its parts is there, in order.
 * Punctuation at the ends of a part is not held against it.
 */
export function quoteInText(quote: string, retrieved: string): boolean {
  const text = normal(retrieved);
  const parts = normal(quote)
    .split(/\[\s*\.\.\.\s*\]|\.\.\./)
    .map((part) => part.replace(/^[\s.,;:!?"'()-]+|[\s.,;:!?"'()-]+$/g, ""))
    .filter((part) => part.length > 0);
  if (parts.length === 0 || text.length === 0) return false;
  let from = 0;
  for (const part of parts) {
    const at = text.indexOf(part, from);
    if (at < 0) return false;
    from = at + part.length;
  }
  return true;
}

export interface QuoteResult {
  messageId: number;
  quote: string;
  found: boolean;
}

export interface QuoteCheck {
  /** How many answers cite the source. */
  cited: number;
  /** Each quote those answers attribute to it, once. */
  quotes: QuoteResult[];
}

/** The quote check for source `n`: every quote the answers attribute to it, looked up in
 *  the text the model was given of it. */
export function checkQuotes(answers: { id: number; text: string }[], n: number, retrieved: string): QuoteCheck {
  const result: QuoteCheck = { cited: 0, quotes: [] };
  const seen = new Set<string>();
  for (const answer of answers) {
    if (!citedIn(answer.text, n)) continue;
    result.cited += 1;
    for (const quote of quotesFor(answer.text, n)) {
      const key = normal(quote);
      if (seen.has(key)) continue;
      seen.add(key);
      result.quotes.push({ messageId: answer.id, quote, found: quoteInText(quote, retrieved) });
    }
  }
  return result;
}
