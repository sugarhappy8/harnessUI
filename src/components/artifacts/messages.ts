import type { Problem, ProblemType } from "./types";

// What a preview frame may post to this window. The frame holds a page the model wrote,
// so a message is whatever that page chose to send: every field is checked, every text is
// cut, and nothing of it is ever used as anything but text.

/** Problems kept for one preview. */
export const MAX_PROBLEMS = 10;
/** Characters kept of one message. */
export const MAX_MESSAGE_CHARS = 300;
/** Messages looked at from one page before the rest is ignored. */
export const MAX_MESSAGES = 200;
/** The highest line or column believed. */
const MAX_POSITION = 9_999_999;

const TYPES: readonly ProblemType[] = ["error", "rejection", "console", "blocked", "diagram", "svg"];

export type FrameMessage =
  | { kind: "ready" }
  | { kind: "problem"; problem: Problem }
  /** The page says it cannot draw this kind on this system. */
  | { kind: "unavailable" };

function position(value: unknown): number | null {
  return typeof value === "number" && Number.isInteger(value) && value >= 1 && value <= MAX_POSITION ? value : null;
}

/** A message's text as one line of at most `MAX_MESSAGE_CHARS` characters. */
export function cleanText(value: unknown): string {
  if (typeof value !== "string") return "";
  // Cut before anything else: a page can post megabytes.
  const head = value.slice(0, MAX_MESSAGE_CHARS * 8);
  const oneLine = head.replace(/\s+/g, " ").trim();
  return oneLine.length > MAX_MESSAGE_CHARS ? `${oneLine.slice(0, MAX_MESSAGE_CHARS - 1)}…` : oneLine;
}

/**
 * Reads one message of the preview of `id`, `version`. Null for anything that is not
 * exactly a message of that preview.
 */
export function parseFrameMessage(data: unknown, id: number, version: number): FrameMessage | null {
  if (typeof data !== "object" || data === null || Array.isArray(data)) return null;
  const m = data as Record<string, unknown>;
  if (m.source !== "artifact" || m.id !== id || m.version !== version) return null;
  if (m.kind === "ready") return { kind: "ready" };
  if (m.kind === "unavailable") return { kind: "unavailable" };
  if (m.kind !== "problem") return null;
  const message = cleanText(m.message);
  if (message === "") return null;
  const type = TYPES.find((t) => t === m.type) ?? "error";
  const line = position(m.line);
  return { kind: "problem", problem: { type, message, line, col: line === null ? null : position(m.col) } };
}

/** Adds a problem to a list unless it is already there or the list is full. */
export function addProblem(list: Problem[], p: Problem): Problem[] {
  if (list.length >= MAX_PROBLEMS) return list;
  if (list.some((q) => q.type === p.type && q.message === p.message && q.line === p.line && q.col === p.col)) {
    return list;
  }
  return [...list, p];
}

const LABEL: Record<ProblemType, string> = {
  error: "",
  rejection: "Unhandled promise rejection: ",
  console: "console.error: ",
  blocked: "Blocked by the preview's rules: ",
  diagram: "",
  svg: "",
};

/** One problem as a line of text: `14:9 message`, `line 14: message` or `message`. */
export function problemLine(p: Problem): string {
  const text = `${LABEL[p.type]}${p.message}`;
  if (p.line !== null && p.col !== null) return `${p.line}:${p.col} ${text}`;
  if (p.line !== null) return `line ${p.line}: ${text}`;
  return text;
}

/** The message "Ask the model to fix these" puts into the message box. */
export function fixRequest(title: string, id: number, version: number, problems: Problem[]): string {
  const lines = problems.slice(0, MAX_PROBLEMS).map((p) => `- ${problemLine(p)}`);
  const what = problems.length === 1 ? "this problem" : "these problems";
  return [
    `The preview of "${title}" (artifact #${id}, version ${version}) reported ${what}:`,
    ...lines,
    "Please fix it and save a corrected version of the artifact.",
  ].join("\n");
}
